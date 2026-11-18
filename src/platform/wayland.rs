use dbus::{
    Message, Path as BusPath,
    arg::{PropMap, Variant},
    blocking::{BlockingSender, Connection},
    message::MatchRule,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use url::Url;

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";

pub fn scan() -> Result<Vec<String>, String> {
    let connection = Connection::new_session().map_err(|e| format!("无法连接桌面会话：{e}"))?;
    let token = format!("otpleaf_{}", uuid::Uuid::new_v4().simple());
    let sender = connection
        .unique_name()
        .trim_start_matches(':')
        .replace('.', "_");
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    // Subscribe before calling Screenshot: a fast backend can send Response
    // before the synchronous method has returned its request handle.
    let predicted = MatchRule::new_signal(REQUEST, "Response")
        .with_sender(SERVICE)
        .with_path(request_path);
    connection
        .add_match_no_cb(&predicted.match_str())
        .map_err(|e| format!("无法监听桌面截图响应：{e}"))?;

    let timeout = Duration::from_secs(10);
    let mut options = PropMap::new();
    options.insert("handle_token".into(), Variant(Box::new(token)));
    options.insert("interactive".into(), Variant(Box::new(true)));
    let call = Message::new_method_call(
        SERVICE,
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Screenshot",
        "Screenshot",
    )?
    .append2("", options);
    let acknowledgement = connection
        .send_with_reply_and_block(call, timeout)
        .map_err(|e| format!("桌面截图服务不可用，请从二维码图片导入：{e}"))?;
    let handle = acknowledgement
        .read1::<BusPath>()
        .map_err(|_| "桌面截图服务未返回请求对象")?
        .into_static();
    let owner = acknowledgement
        .sender()
        .ok_or("桌面截图服务缺少发送者信息")?
        .into_static();
    // A legacy portal may return a different request object. Match the path
    // it acknowledged and its unique bus name before dispatching queued signals.
    let rule = MatchRule::new_signal(REQUEST, "Response")
        .with_strict_sender(owner.clone())
        .with_path(handle.clone());
    let (reply, response) = mpsc::channel();
    connection
        .add_match(rule, move |(status, result): (u32, PropMap), _, _| {
            let uri = result
                .get("uri")
                .and_then(|v| v.0.as_str())
                .map(str::to_owned);
            let _ = reply.send((status, uri));
            false
        })
        .map_err(|e| format!("无法监听桌面截图响应：{e}"))?;
    let request = connection.with_proxy(owner, handle, timeout);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok((status, uri)) = response.try_recv() {
            match status {
                0 => {
                    let path = uri
                        .as_deref()
                        .and_then(|uri| Url::parse(uri).ok())
                        .and_then(|url| url.to_file_path().ok())
                        .ok_or("桌面截图服务未返回本地图片")?;
                    return super::qr::from_image(&path);
                }
                1 => return Err("已取消屏幕截图".into()),
                _ => return Err("桌面截图失败，请检查屏幕共享权限".into()),
            }
        }
        if Instant::now() >= deadline {
            let _: Result<(), _> = request.method_call(REQUEST, "Close", ());
            return Err("桌面截图请求超时".into());
        }
        connection
            .process(Duration::from_millis(250))
            .map_err(|e| format!("桌面截图失败：{e}"))?;
    }
}
