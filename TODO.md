# Windows 版 0→1 进度

## 已完成
- [x] 仓库骨架、Cargo 依赖、manifest（PMv2 + Common Controls 6）、图标、版本信息（build.rs 按 Cargo 版本生成）
- [x] 纯逻辑（kit）：几何、图片编解码、标注模型、颜色、段落合并、取色、翻译接口与缓存、设置存储、文件命名
- [x] 渲染（render）：tiny-skia 画布、系统字体排版（中英日韩回退）、标注/译文渲染、马赛克效果
- [x] OCR（ocr）：tract 跑 PP-OCRv6（det tiny + rec small），与 RapidOCR 结果一致；模型下载与 SHA-256 校验；不需要 onnxruntime.dll
- [x] 截图界面状态机（ui）：选区、7 个标注工具、撤销重做、悬停卡片、单键改键、样式条、放大镜、OCR 面板、翻译、导出
- [x] Win32 层（win）：托盘、全局热键、多屏冻结截图、覆盖层、隐藏输入框 + IME、剪贴板、贴图、HUD、设置窗口、DPAPI、开机启动、WinSparkle、首次下载提示
- [x] Win32 代码审查（修了 14 个问题，见提交说明）
- [x] 覆盖层重绘性能：4K 全屏重绘约 7 ms（M4），不需要更细的脏区
- [x] 截图状态机场景测试（22 个），全部测试 78 个
- [x] `--check` 自检；`--ui-demo`；`--bench-render`；`--ocr`
- [x] 体积：exe 12.7 MB（tract 用 opt-level s），另下载模型 23 MB
- [x] 构建脚本（Mac 交叉编译）、Inno Setup 安装包脚本、CI 发版 workflow、模型镜像 workflow、appcast 签名脚本（已用真实密钥校验公钥匹配）
- [x] 文档：AGENTS.md、README

## Windows 虚拟机实测（UTM，Windows 11 ARM64）
- [x] `--check all` 9 项全过（含截屏、剪贴板、DPAPI、热键、OCR）
- [x] `--e2e` 全过：启动、首次下载模型、热键截图、覆盖层拿到焦点、拼音输入、复制尺寸、保存、OCR、贴图、设置窗口
- [x] 修复：输入法候选窗跑到屏幕左上角（隐藏输入框改为放在光标处的透明分层窗口）；正在输入的拼音不显示（现在带下划线画在光标处）

## 用户试用反馈（第一轮）
- [x] 设置 API Key 后点“测试连接”、截图里翻译会闪退：Windows 上 ureq 未配置系统根证书导致 panic → 统一 TLS 配置，并加回归自检
- [x] 设置界面是 Windows 默认控件：改为自绘，和截图工具栏同一风格（浅色 / 深色）
- [x] 后台任务防崩溃 + crash.log
- [x] 贴图上直接选中、复制文字（和 Mac 一致：拖选、双击选词、三击选行、Ctrl+A / Ctrl+C、右键「复制选中文字」，悬停文字时显示 I 形光标；第一次 Esc 只取消选择）

## 发布与素材
- [x] 0.1.0 发布（GitHub Actions：x64 / arm64 安装包 + appcast），模型镜像到 `models-v1`
- [x] 仓库主页指向官网 shotlate.pages.dev；官网加上 Windows 版（首页、下载页、常见问题、使用手册）
- [x] README 的动图和设置截图换成在 Windows 11 上录制的（`scripts/vm/record-demo.sh`）
- [x] 内存：常驻从约 90 MB 降到约 22 MB，识别后的空闲占用从约 190 MB 降到约 40 MB（字体改为内存映射；OCR 引擎不再启动预热，空闲 60 秒释放）

## 需要用户操作
- [ ] 在真实 Windows 上试用 `dist/Shotlate.exe` 或 CI 产出的安装包

## 无法在本机验证（需要 Windows）
- 托盘右键菜单、贴图滚轮缩放、悬停卡片改键、翻译（需要 API Key）的实际点按
- 搜狗、日文输入法（虚拟机里只测了微软拼音）
- 混合 DPI 多屏（虚拟机只有一块屏）、深色模式下 OCR 面板配色、设置窗口跨屏 DPI 变化
- MSVC 目标编译（只在 CI 上）、WinSparkle 实际更新流程、Inno Setup 打包
- 开机时 Explorer 未就绪导致托盘图标添加失败（有 TaskbarCreated 兜底，无重试）

## 第二版
- 长截图、扫码、贴图再标注、贴图翻译、延时截图、从剪贴板贴图、选区放大镜以外的取色功能保持现状
