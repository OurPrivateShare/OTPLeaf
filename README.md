# OTPLeaf

OTPLeaf 是一个用 Rust 和 Slint 编写的便携桌面 TOTP 验证器，适用于 Windows 和 Linux。它使用系统时间在本地生成验证码，运行时可以断网。条目和密钥以明文 JSON 保存在程序旁边。

## 使用

把程序放在当前用户可以写入的文件夹，双击 Windows 版 `OTPLeaf.exe`。Linux x64 版解开 tar.gz 后，在桌面会话中运行 `./OTPLeaf`。预构建的 Linux 版本以 glibc 2.35 为基线，使用系统的 Fontconfig 和中文字体。首次启动会创建空列表。

- **添加或编辑**　在主窗口点击「添加条目」，在弹窗中输入名称和 Base32 密钥。密钥可以带空格，保存时会统一为紧凑的大写格式。默认使用 6 位、30 秒和 SHA1，也可以选择 7 或 8 位、调整周期，或改用 SHA256、SHA512。铅笔按钮打开编辑弹窗，垃圾桶按钮打开删除确认。
- **查看与复制**　按名称搜索，结果数量显示在搜索框右侧。验证码左侧显示遮挡后的密钥，长密钥保留首尾各 4 位，短密钥会保留遮挡。点击密钥可复制完整内容，点击验证码可复制按当前时间生成的纯数字验证码。
- **导入**　点击「导入条目」可在弹窗中粘贴 `otpauth://totp/…` 链接。「打开图片」支持 PNG、JPEG、BMP 和 WebP。「识别屏幕」会在截图期间隐藏主窗口。识别成功后会打开条目弹窗。遇到同一张图中的多个不同链接时逐项确认，取消可结束本次导入。
- **托盘**　关闭主窗口后程序驻留托盘。托盘菜单提供「显示窗口」「识别屏幕」「复制验证码」和「退出」。复制子菜单按条目名称列出选项。

## 便携数据

```text
OTPLeaf.exe                # Linux 下为 OTPLeaf
data/
  accounts.json            # 条目与 Base32 密钥
  .otpleaf.lock            # 运行期间使用的文件锁
```

数据路径以可执行文件的位置为准。移动、备份或恢复时，先从托盘或主窗口底部退出，再复制程序和 `data/` 目录。Windows 和 Linux 使用相同的 JSON 格式。持有 `accounts.json` 的人可以生成这些验证码，备份也请按密钥保管。

程序通过同目录临时文件替换旧 JSON。多个实例同时使用同一数据目录时，后启动的实例会提示文件占用。如果 JSON 已损坏，启动会显示出错的文件并保留原内容。

## 技术方案与目录

主程序使用 Slint 的 winit 后端和软件渲染器。Rust 业务层按 RFC 6238 计算时间步和 HMAC，使用 Base32 解码密钥，按条目配置截取验证码。列表用同一个时间戳计算显示值和倒计时。托盘与主窗口共用复制逻辑。二维码解码和屏幕采集在后台线程运行，结果送回 UI 线程供用户确认。

```text
Cargo.toml / Cargo.lock    依赖与锁定版本
build.rs                   编译 Slint 界面
assets/                    内嵌窗口图标和 Windows 文件图标
ui/
  main.slint               列表、条目弹窗、链接导入和删除确认
  widgets.slint            通用控件、配色和图标
src/
  main.rs                  入口与启动错误提示
  app.rs                   条目操作、筛选、倒计时、复制和导入流程
  core.rs                  条目模型、参数校验、TOTP、otpauth 解析
  storage.rs               可执行文件旁的 JSON 存储和文件锁
  platform/
    mod.rs                 托盘动作和平台选择
    qr.rs                  图片解码和多显示器扫描
    wayland.rs             Wayland 桌面截图门户
    windows_tray.rs        Windows 通知区域
    linux_tray.rs          Linux StatusNotifierItem
    fixtures/totp-pair.png  使用公开示例密钥的二维码测试图片
```

算法与界面框架的资料见 [RFC 6238](https://www.rfc-editor.org/rfc/rfc6238) 和 [Slint Rust API](https://docs.slint.dev/latest/docs/rust/slint/)。

## 从源码运行

使用 Rust stable 工具链，本项目的构建验证使用 Rust 1.99.1。Windows 使用 MSVC 工具链及 Visual Studio 的「使用 C++ 的桌面开发」组件。Linux 需要 C/C++ 构建工具、pkg-config、Fontconfig 开发库和桌面运行环境。例如在 Ubuntu 22.04 上可准备以下依赖。

```bash
sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-dev libwayland-dev libxcb1-dev libxcb-randr0-dev libxcb-render0-dev fonts-noto-cjk
```

在项目根目录运行以下命令。

```sh
cargo run --locked
cargo test --locked
cargo build --locked --release
```

默认发布构建输出到 `target/release/`。首次构建需要下载依赖。若目标机器需要离线编译，可在有网络的构建机先执行 `cargo fetch --locked`，将依赖缓存备好，再使用 `cargo build --locked --release --offline`。运行已构建的程序无需 Cargo。

## Linux 桌面集成

- **系统托盘** 使用 StatusNotifierItem，需要桌面面板提供相应支持。桌面没有可用托盘时，关闭操作会将程序最小化到任务栏，可从主窗口底部退出。
- **屏幕识别** 在 X11 会话中读取显示器图像。在 Wayland 会话中通过 XDG Desktop Portal 的 Screenshot 接口请求截图，按桌面提示选择或授权。门户可能生成本地截图文件，其位置和保留时间由桌面后端决定。
- **剪贴板与文件选择** 使用桌面剪贴板服务和系统文件选择器。Wayland 的剪贴板取决于合成器是否支持 data-control 或是否启用了 XWayland。文件选择器需要桌面文件门户或 Zenity。
