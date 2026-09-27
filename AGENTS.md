# AGENTS.md

写给在这个仓库里工作的 AI 编码代理（Claude Code、Codex 等）的说明：常用命令、整体结构、已经定下的取舍和容易踩的坑。README 面向用户，这里只写改代码时需要知道、但从单个文件看不出来的东西。

## 和 Mac 版的关系

- Mac 版在 `/Users/lin/code/my/shotlate`（GitHub `zuijiaosy/shotlate`，Swift + AppKit）。**产品规格以 Mac 版的 AGENTS.md 为准**：做什么、不做什么、工具栏交互、快捷键、OCR 不自动复制、设置没有保存按钮、默认保存到 Downloads，都照那边来。这里只写 Windows 特有的部分。
- 两边不共享代码。Mac 版 `ShotlateCore` 和截图界面的逻辑在这里用 Rust 重写：`kit/textblocks.rs`、`kit/colorsample.rs`、`kit/translator.rs`、`ui/capture.rs`（CaptureView）、`ui/chrome.rs`（Chrome.swift）、`render/content.rs`（ContentRenderer）都是逐行对照移植的，改交互时两边一起看。
- 调研和设计文档：https://claude.ai/artifact/D71Jnif8SrNkHAdpEAh8Rd

## 常用命令

开发机是 macOS：纯逻辑、渲染、OCR 都能在 Mac 上跑和测试，Win32 层只能编译检查。

```bash
cargo test --release                                  # 单元测试（纯逻辑、渲染、截图状态机、OCR 的纯函数部分）
SHOTLATE_MODELS=<模型目录> cargo test --release        # 连 OCR 准确率测试一起跑（mixed.png / memory.png）
cargo check --target x86_64-pc-windows-gnu            # 在 Mac 上检查 Windows 代码能否编译
cargo run --release -- --ui-demo out/                 # 离屏驱动截图界面，每一步输出一张 PNG（加 --scale 2 看高分屏）
cargo run --release -- --check all out/               # 自检，逐条 PASS / FAIL / SKIP；Windows 上还会测截屏、剪贴板、DPAPI、热键
cargo run --release -- --ocr tests/data/mixed.png --models <模型目录>   # 识别一张图，打印每行和耗时
Shotlate.exe --ocr-memory tests/data/mixed.png                      # OCR 引擎各阶段（加载、各计划、识别、释放）的内存和耗时（Windows）
cargo run --release -- --bench-render                 # 4K 下覆盖层重绘耗时
scripts/check.sh                                      # 上面几项的组合：测试 + Windows 编译检查 + UI 演示
scripts/build-windows.sh                              # Mac 上用 mingw 交叉编译出 dist/Shotlate.exe（x64，拿去 Windows 上试）
python3 scripts/appcast.py --check                    # 确认 EdDSA 私钥和应用里的公钥是一对（需要 pip install cryptography）
```

改完代码的标准验证：`cargo build --release` 和 `cargo check --target x86_64-pc-windows-gnu` 都没有警告 → `cargo test --release` → `--check all`。改了界面时，看一眼 `--ui-demo` 输出的 PNG。

交叉编译需要 `brew install mingw-w64` 和 `rustup target add x86_64-pc-windows-gnu`。MSVC 目标在 Mac 上编不了（tract-linalg 的构建脚本要 `lib.exe`），正式版本由 CI 在 Windows 上用 MSVC 编 x64 和 ARM64。

## 在 Windows 虚拟机里测试

本机装了 UTM，虚拟机「Shotlate Win11」：Windows 11 ARM64 简体中文、微软拼音，账户 tester / shotlate，自动登录。x64 的 exe 在 ARM 上转译运行。

```bash
scripts/vm/install.sh           # 在虚拟机里用 Inno Setup 打正式安装包并装上（桌面图标、保留设置；--fresh 清空后装）
scripts/vm/test.sh              # 交叉编译 → 拷进虚拟机 → --check all + --e2e → 日志和截图拉回 target/vm-e2e/
scripts/vm/gx 'dir C:\shotlate' # 在虚拟机里执行命令（SYSTEM 身份，没有桌面）
scripts/vm/guirun '<命令行>'     # 在 tester 的桌面上执行（计划任务），GUI 相关都要走这个
scripts/vm/record-demo.sh       # 录 README / 官网素材（见下方「README 素材」）
scripts/vm/vmshot out.png       # 从 Mac 截虚拟机窗口
scripts/vm/setup.sh             # 从零重建虚拟机（下载微软官方镜像 + 无人值守安装，约 30 分钟）
```

`--e2e` 用 SendInput 像真人一样操作已安装的应用：热键截图、覆盖层是否拿到焦点、拖选、画标注、拼音输入、Ctrl+C 后剪贴板尺寸、Ctrl+S、首次下载模型、OCR、贴图和贴图上的文字选择、设置窗口，每步截图。可以只跑一段：`--e2e <输出目录> pin`（段名 capture / save / ocr / pin / settings / translate）。被测应用带着 `SHOTLATE_TRACE=1` 启动，`util::trace` 会把调试信息追加到 `%TEMP%\shotlate-trace.log`（tester 的是 `C:\Users\tester\AppData\Local\Temp\shotlate-trace.log`），平时不写。虚拟机只有一块 1024×768、100% 缩放的屏幕，混合 DPI 多屏还得在真机上测。guest agent 的输出是 GBK，e2e.log 是 UTF-8，直接用 `utmctl file pull` 拉。

## 结构

```
src/kit/      纯逻辑，任何系统都能测：几何、图片编解码、标注模型、颜色、段落合并、取色、翻译接口与缓存、设置存储、文件命名
src/render/   tiny-skia 软件渲染：画布（变换、裁剪、圆角、阴影）、系统字体排版（中英日韩逐字回退）、标注/译文渲染、马赛克
src/ocr/      tract 跑 PP-OCRv6：检测 → 裁剪 → 识别 → CTC 解码；models.rs 管下载和 SHA-256 校验
src/ui/       平台无关的界面：capture.rs 是截图覆盖层的状态机（输入事件进，Effect 出），chrome.rs 是各种面板的布局和绘制
src/win/      Win32 层（只在 Windows 编译）：托盘、热键、覆盖层窗口、贴图、HUD、设置窗口、剪贴板、DPAPI、开机启动、WinSparkle
src/dev/      命令行开发工具：--ui-demo、--check、--ocr、--bench-render
installer/    Inno Setup 脚本；scripts/ 构建和 appcast 签名；.github/workflows/ 发版和模型镜像
tests/data/   OCR 测试图（mixed.png 有标准答案 mixed.txt）
```

跨文件才看得出的关系：

- **一次截图 = overlay::Session + 每块屏幕一个 Overlay（窗口 + CaptureView）**。CaptureView 不碰任何系统 API：win 层把 WM_* 消息换成 `mouse_down / mouse_move / key_down / wheel / tick` 等调用，调用完取 `drain_effects()`，由 `overlay::apply` 执行（剪贴板、保存、贴图、OCR、翻译、取色、输入框）。同一时刻只有拥有选区的屏幕能操作（`Session.owner`）。
- **重绘**：CaptureView 每次状态变化记脏，`take_dirty()` 给出要重绘的像素矩形（上一帧和这一帧所有面板、选区、标注的并集），WM_PAINT 调 `render_region` 画那一块，再用 SetDIBitsToDevice 贴上去。截图底图只是整块拷贝，所以 4K 全屏重绘在 M4 上约 7 ms。
- **屏幕上和导出的图是同一个渲染器**：`render::content::ContentRenderer`，导出按像素密度重新画一遍选区（`CaptureView::export_image`）。
- **文字输入**：标注文字不用自绘输入框，而是在覆盖层里放一个原生 EDIT 子窗口接收键盘和输入法：它是分层子窗口（透明度 1/255，看不见），只有 2 px 宽，**就放在画出来的光标位置**。微软拼音这类 TSF 输入法按 EDIT 自己的位置放候选窗，不理 ImmSetCompositionWindow，所以 EDIT 不能挪到屏幕外（试过，候选窗会跑到屏幕左上角）。文字、选区和正在输入的拼音（ImmGetCompositionStringW）同步回 CaptureView 由它画出来，拼音带下划线。覆盖层本身用 `ImmAssociateContext(hwnd, NULL)` 关掉输入法，否则开着中文输入法时按 `1` 选不了工具。
- **OCR 面板**的文字区是真的 EDIT 控件（可编辑、可选中），位置由 `Effect::OcrPanel` 告诉 win 层；复制按钮复制的是 EDIT 里改过的文字。
- **Re-entrancy**：很多 Win32 调用会同步给自己的窗口发消息（创建、销毁、SetFocus、模态对话框）。会话和贴图状态放在 thread_local RefCell 里，只在很短的调用里借用，副作用在借用结束后执行；窗口过程借不到就交给 DefWindowProc。`borrow_mut()` 冲突会 panic；release 版虽是 `panic = "unwind"`，但窗口过程里的 panic 过不了 FFI 边界，照样闪退，改 win 层时要特别注意。
- **设置窗口**和截图界面一样是自绘的：`ui/settings_view.rs` 负责布局、绘制和交互（左侧菜单、圆角卡片、开关、分段控件、快捷键录制框），可以在 Mac 上用 `--ui-demo` 出图；`win/settings_window.rs` 只是外壳。只有 API Key、Base URL、模型三个输入框是原生 EDIT（无边框，放在画出来的输入框里）。快捷键是点录制框后直接按组合键，录制期间暂停全局热键。
- **HTTP**：所有 ureq 客户端都必须用 `kit::http::tls()`。Windows 上 ureq 走 Schannel，不指定系统根证书会直接 panic——翻译和“测试连接”闪退就是这个原因。`--check translate-network` 会真的发一次请求（假 Key，期望 401）来防止回归。
- **崩溃**：release 用 `panic = "unwind"`，后台任务（OCR、翻译、测试连接）包在 `util::guarded` 里，出错只变成一条错误提示；panic 信息会写到 `%APPDATA%\Shotlate\crash.log`，用户报闪退时先要这个文件。
- **设置**：`kit::settings` 存在 `%APPDATA%\Shotlate\settings.json`，每次修改立即写入；没调用 `settings::init` 时（测试、开发工具）只在内存里，不会碰真实配置；测试里是线程局部的，互不干扰。
- **API Key**：DPAPI 加密后存 `%APPDATA%\Shotlate\api-key`（`win/secret.rs`）。debug 构建只读环境变量 `DEEPSEEK_API_KEY`，从不碰真实文件。
- **模型**：`%LOCALAPPDATA%\Shotlate\models\`，首次打开时询问下载（`app::first_launch`），ModelScope 为主、本仓库 Release `models-v1` 为备用，SHA-256 写死在 `ocr/models.rs`。没下载时截图标注照常，按 OCR / 翻译时再提示。所有识别都走 `app::recognize`：第一次用时才加载模型，空闲 60 秒后释放（见下方「内存」）。

## 已经定下来的（改之前先确认）

- **Rust，不用跨平台框架**；界面全部自绘（tiny-skia + 系统字体），Win32 只负责窗口、输入和系统功能。
- **OCR 用 tract 跑 RapidOCR 的 PP-OCRv6 ONNX 模型**：检测 `PP-OCRv6_det_tiny`（1.8 MB）+ 识别 `PP-OCRv6_rec_small`（21.2 MB）。识别 tiny 认不出日文，不要换。**不需要 onnxruntime.dll**（原计划要下载 dll，改用纯 Rust 的 tract 后省掉了），只下载两个模型，约 23 MB。字符表在识别模型的元数据里。
- tract 只能按具体输入尺寸优化：检测按图片尺寸缓存最近 4 个计划，识别按宽度档位（160…2560）缓存，太宽的按 1280 取整。符号尺寸的识别慢 3 倍，别用。
- 体积：tract 的图处理代码用 `opt-level = "s"`（Cargo.toml 末尾），exe 从约 20 MB 降到约 13 MB（改成 panic = unwind 后约 14.7 MB），识别速度基本不变；`"z"` 能到更小但识别慢 30%，没用。
- **翻译用 DeepSeek（deepseek-flash）**，接口和缓存与 Mac 版一致，只发送识别出的文字。设置 → 翻译里的说明文字：只截图、标注、识别不用填；需要翻译时登录 platform.deepseek.com，充值最少 1 元，在「API Keys」创建并粘贴。
- 默认截图热键 `Alt+Shift+A`（Alt+A 是微信截图、Ctrl+Alt+A 是 QQ 截图），隐藏/显示贴图 `Alt+Shift+H`。不抢 PrtScn。Mac 的 ⌘ 一律对应 Ctrl；重做同时支持 `Ctrl+Y` 和 `Ctrl+Shift+Z`；工具单键（`1`–`7`、X、Y、T）和 Mac 一样，可在悬停卡片上改。
- 第一版范围：截图 → 7 个标注 → 复制 / 保存 / 贴图（含贴图上直接选中、复制文字）/ OCR / 翻译，外加托盘、设置、自动更新。长截图、扫码、贴图再标注、贴图翻译、延时截图、剪贴板贴图放第二版（工具栏上没有长截图按钮）。
- 安装：Inno Setup，装到 `%LOCALAPPDATA%\Programs\Shotlate`，不要管理员权限、没有向导页。支持 Windows 10 2004 及以上，x64 和 ARM64 各一个安装包。
- 自动更新：WinSparkle 0.9（安装包里带 WinSparkle.dll，开发构建没有，更新功能就不出现）。appcast 用和 Mac 版 Sparkle **同一把 EdDSA 密钥**签名，公钥在 `win/updater.rs`，CI 会检查私钥和它是一对。

## 发布与仓库

- 本地仓库在 `/Users/lin/code/my/shotlate-win`，GitHub 远程计划为 `zuijiaosy/shotlate-win`（还没建；**创建远程、提交和推送都要等用户明确要求**）。
- 推送到 `main` 会自动发版（`.github/workflows/release.yml`）：测试 → MSVC 编 x64 / ARM64 → `--check all` → 打包 WinSparkle → Inno Setup → EdDSA 签名写 appcast → GitHub Release。版本号主、次版本取自 Cargo.toml，补丁号在上一个同系列标签上加一。只改文档的提交写 `[skip release]`。需要 Secret `SPARKLE_ED_PRIVATE_KEY`（和 Mac 仓库同一个值）。
- 第一次发版前手动跑一次 `Mirror models` workflow，把模型复制到 Release `models-v1` 作为备用下载源。
- 代码签名：暂时不签，用户首次运行会看到 SmartScreen 提示；发过几个版本后申请 SignPath Foundation 的免费开源签名。Windows 截屏不需要任何权限，所以签名只影响首次运行的提示。
- 提交信息用中文 conventional commits；正文里的 `- ` 列表会被收进发布说明，写成给用户看的变化；结尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`。

## README 素材

`docs/images/capture-flow.gif` 和 `settings.gif` 由 `scripts/vm/record-demo.sh` 生成：在虚拟机里用已安装的 Shotlate 跑 `--e2e <目录> demo`（记事本打开一段说明文字 → 热键截图 → 选中窗口 → 框选 → 标注 → X 识别 → T 贴图并选中文字 → 打开设置逐页截图），每一帧都画上鼠标指针，最后用 ffmpeg 合成。同时在 `target/demo/` 输出官网用的 `capture-flow.mp4` 和 `capture-flow-poster.webp`，复制到官网的 `public/shots/win-capture-flow.*`。

- 录制时会把虚拟机里的截图快捷键临时改回默认的 `Alt+Shift+A`（备份为 `settings.json.bak`），脚本退出时（包括被中断）用 `trap` 恢复。
- `demo` 段只在显式指定时运行，不属于 `--e2e` 的默认测试。坐标按虚拟机 1024×768 的屏幕写死。
- 虚拟机访问不了 github.com（能访问 ModelScope），要装 CI 发布的安装包，先在 Mac 上 `gh release download` 再 `utmctl file push`。

## 容易踩的坑

- 用 guirun 启动常驻的 Shotlate 时要写 `explorer.exe "<路径>"`：run.cmd 把输出重定向到 last-run.txt，直接启动或 `cmd /c start` 会让应用继承这个文件句柄，之后每次 guirun 都因为写不了这个文件而什么都不执行。
- **内存**：0.1.1 原生 arm64 实测工作集：常驻约 16 MB，截图后约 18 MB，识别文字时约 110 MB，空闲 60 秒后约 25 MB（x64 在 ARM 虚拟机里仿真运行时各多出 10–20 MB）。两处决定了这个数字，改动时用 `Shotlate.exe --ocr-memory <图片>` 和 `scripts/vm/gx` 里的 `Get-Process` 复测：
  - 字体：回退链里有好几个 10–20 MB 的 CJK 字体（微软雅黑、Malgun、Yu Gothic、宋体），`render::text` 用内存映射加载（`font_bytes`），不要改回 `fs::read`，否则常驻和截图后各多出约 70 MB 私有内存。
  - OCR 引擎：优化后的计划很占内存（检测器每个尺寸约 34 MB，识别器每个宽度桶约 22 MB），而加载模型 + 编译计划总共不到一秒，远小于识别本身，所以不在启动时预热，识别完空闲 60 秒就释放（`app::ENGINE_IDLE`）。启动预热对识别速度几乎没有帮助：检测器计划按选区尺寸编译，预热的 1920×1088 很少用得上。

- **`match` 里没导入的 Win32 常量会变成通配绑定**，吞掉后面所有分支。main.rs 里 `#![deny(unreachable_patterns, non_snake_case)]` 把这种情况变成编译错误，别删。
- 进程是 Per-Monitor V2 DPI 感知（manifest + main 里的调用），坐标都是物理像素；CaptureView 用点（像素 ÷ 缩放），换算在 win/overlay.rs。
- 冻结截图用 GDI BitBlt（带 CAPTUREBLT，包含分层窗口）；`Windows.Graphics.Capture` 默认有黄框，没用。
- 托盘用 NOTIFYICON_VERSION_4：事件在 lParam 低位，右键只处理 WM_CONTEXTMENU（同时处理 WM_RBUTTONUP 会弹两次菜单），提示文字要 NIF_SHOWTIP。资源管理器重启后靠 `TaskbarCreated` 重新添加图标。
- SysLink 和热键输入框需要 InitCommonControlsEx 注册，manifest 里要有 Common Controls 6。
- 工具栏图标是自己画的矢量路径（`ui/icons.rs`），SF Symbols 的许可不允许用在 Windows。
- 字体：Windows 上用微软雅黑 UI（msyh.ttc 第 2 个字体），缺字时依次回退 Segoe UI、Malgun Gothic、Yu Gothic 等；Mac 上开发时用冬青黑体，所以 Mac 上渲染的 PNG 和 Windows 上字形不完全一样。
- 测试里的“上一次选区”是进程级的，按屏幕名区分；写新测试时用独立的屏幕名，否则并行测试会互相覆盖。
- 界面文案、README、提交信息用中文；代码注释用英文，简短，写“为什么”。
