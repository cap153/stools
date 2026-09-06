# stools

基于 [Slint](https://slint.dev/) 的极简 Fuzzel 风格应用启动器。

[English](README.md) | 简体中文

<p align="center">
  <img src="assets/preview.jpg" width="720" alt="stools —— Fuzzel 风格启动器预览">
</p>

- **固定居中窗口** — 无边框，不做动态重排。
- **模糊 + 拼音匹配**（`nucleo-matcher` + `pinyin`），支持多音字，因此 `wyyyy`
  能匹配到网易云音乐。
- **应用与二进制** — 除 `.desktop` 条目外，还检索常见 PATH 目录中的可执行文件。
  二进制排序低于应用，并以灰色副标题显示所在目录。
- **MRU 历史** — 最近与常用条目自动置顶
  （`~/.local/share/stools/history.json`）。
- **渲染引擎可配置** — 软件渲染（约 30MB，不占用显卡）或 OpenGL 硬件加速；
  Linux 默认 OpenGL，Windows 默认软件渲染。
- **Linux** — 单次运行、无状态；索引缓存在 `~/.cache`，冷启动近乎瞬时。无托盘、
  无内置热键，由窗口管理器绑定按键。
- **Windows** — 托盘常驻守护进程，支持全局热键（默认 **Alt+A**）。
- 不支持 macOS。

## 安装

- **Arch Linux** — 从 AUR 安装：`yay -S stools-bin`（或 `paru -S stools-bin`）。
- **其他 Linux / Windows** — 从
  [GitHub Releases](https://github.com/cap153/stools/releases) 下载预编译二进制。
- **手动编译** — `cargo build --release`（产物为 `target/release/stools`）。

## Linux 配置

stools 是无状态的：绑定一个按键，并让窗口管理器把窗口浮动居中即可。

```ini
# ~/.config/sway/config
for_window [app_id="stools"] floating enable
for_window [app_id="stools"] move position center
bindsym $mod+space exec stools
```

| 按键 | 行为 |
|---|---|
| `↑` / `↓` | 移动选中项 |
| `Enter` | 启动并退出 |
| `Esc` | 退出 |
| 输入 | 实时模糊 / 拼音过滤，命中字符高亮 |

首次运行会扫描并写入缓存；之后直接载入缓存并在后台刷新，因此新安装的应用会
自动出现。

## Windows

以后台托盘守护进程方式运行：

- **Alt+A**（或左键单击托盘图标）唤出窗口。
- **Esc** 隐藏；再次唤出时会选中已输入的文本。
- 托盘菜单：**显示**、**重载配置**、**打开配置目录**、**退出**。

扫描开始菜单的 `.lnk` 文件，通过 shell 启动，图标也从 shell 读取（`.lnk`、
`.exe`、`.url`）。配置中的 `path` 目录会像 Linux 一样被扫描可执行文件。

## 系统快捷操作（`extras/`）

stools 按设计不提供插件系统。把现成的快捷方式放进已扫描的目录即可 —— 无需重启，
索引会在后台刷新。

- **Linux** — `cp extras/linux/*.desktop ~/.local/share/applications/`
  （关机、重启、锁屏、清空/打开回收站；`Name` 与 `Name[zh_CN]` 双语，因此
  `关机` 和 `poweroff` 都能匹配）。
- **Windows** —
  `powershell -ExecutionPolicy Bypass -File extras/windows/install_shortcuts.ps1`
  （在开始菜单的 `stools Commands` 目录下创建快捷方式）。

## 配置

全部配置集中在 `config.toml`，首次运行时生成带注释的默认文件：

| 平台 | 路径 |
|---|---|
| Linux | `~/.config/stools/config.toml` |
| Windows | `%APPDATA%\stools\config.toml` |

每一项都是可选的 —— 删除该文件即恢复内置 Dracula 配色。解析出错时会在 `stderr`
报错并回退到默认值，因此配置写坏也绝不会导致启动器起不来。

```toml
# renderer = "cpu"               # 默认：Linux 为 "gpu" / Windows 为 "cpu"
path = ["$HOME/.cargo/bin"]      # 额外扫描目录（会展开 ~、$VAR、%VAR%）

[keybindings]                    # 无修饰键
tab = "down"
esc = "close"                    # Linux：退出 / Windows：隐藏
Return = "execute"
Up = "up"
Down = "down"

[keybindings.shift]
tab = "up"

[keybindings.alt]
a = "stools"                     # 唤出窗口（Windows 全局热键）

[theme]                          # Fuzzel RRGGBBAA 颜色
background = "282a36dd"
text = "f8f8f2ff"
prompt = "586e75ff"              # ">" 提示符
match = "8be9fdff"               # 命中字符
selection-match = "8be9fdff"     # 选中行中的命中字符
selection = "44475add"
selection-text = "f8f8f2ff"
border = "bd93f9ff"
font = ["ComicShannsMono Nerd Font", "LXGW WenKai GB Screen"]
```

**渲染引擎** — `renderer` 在软件渲染（无 GPU 上下文，内存约 30MB）与 OpenGL 硬件
加速（约多 150MB）之间切换。默认值：Linux 为 `"gpu"` —— 软件渲染器未实现
`border-radius` 与 `clip` 的组合，在 Wayland 下窗口圆角四周会留有瑕疵；Windows 为
`"cpu"`，渲染正常。渲染引擎在进程启动时即已确定，因此 Windows 修改后需重启
stools；Linux 下次呼出即生效。命令行指定 `SLINT_BACKEND=winit-software` /
`=winit-femtovg` 可临时覆盖配置。

**搜索路径** — `path` 会追加到内置目录集合（`~/.local/bin`、`/usr/local/bin`、
`/usr/bin`、`/bin`、`/usr/sbin`、`/sbin`、`/home/linuxbrew/.linuxbrew/bin`）之上，
这些目录同时也会被扫描 `.desktop` 文件（Windows 为 `.exe`/`.bat`/`.cmd`/`.ps1`/
`.lnk`）。`~`、`$VAR`、`%VAR%` 会被展开，不存在或重复的目录会被忽略。命令行参数
会在其之上追加：

```sh
stools "$HOME/.zvm/bin" "/home/linuxbrew/.linuxbrew/bin"
```

修改 `path` 会使缓存失效，下次启动会重新扫描而不是返回过期结果。过长的副标题会
缩写为 `head/.../tail`，选中该行时完整路径会滚动显示。

**按键绑定** — 可用动作：`up`、`down`、`execute`、`close`、`stools`。表名即修饰键
组合：`[keybindings]` 表示无修饰键，或由 `ctrl` / `alt` / `shift` / `super`
（`win`/`meta`/`cmd`）任意组合，用 `_` 或 `+` 连接（使用 `+` 时需加引号）。键名
不区分大小写，既接受 XKB 拼写（`Return`、`Escape` 等），也接受常见别名（`enter`、
`esc` 等）。

**主题** — 使用 Fuzzel 的 `RRGGBBAA` 记法（也接受前导 `#` 及 `RGB`/`RGBA`/
`RRGGBB` 形式），因此现有 Fuzzel 主题可直接粘贴使用。`font` 是优先级列表：取第一
个系统已安装的字体，缺失的字形（例如拉丁等宽字体中的中日韩字符）由系统字体回退
补全。

## 项目结构

```
build.rs               编译 .slint UI
ui/launcher.slint      UI 定义
assets/                icon.svg 及生成的 icon.png / icon.ico
tools/build_icon.py    从 assets/icon.svg 重新生成图标
extras/                示例系统操作（linux .desktop / windows .ps1）
src/
  main.rs              平台分发
  launcher.rs          Slint 模型公共辅助
  core/                config, keybind, theme, matcher, model, indexer,
                       history, search, i18n, path_utils
  platform/            linux.rs, windows.rs（+ windows_icon.rs 读取 shell 图标）
```

## 调试

```sh
STOOLS_DEBUG=1 stools
# [stools] renderer=software
# [stools] load=2.4ms trim=2.4ms new=26.4ms model=37.1ms show=46.2ms apps=159
# [stools] search-rebuild=48.2µs n=10
```

`renderer` = 当前使用的 Slint 渲染器，`load` = 索引载入（命中缓存或全量扫描），
`trim` = 将扫描期临时内存归还内核，`new` = Slint 窗口与渲染器初始化，`model` /
`show` = 构建首屏列表并显示窗口，`search-rebuild` = 每次按键的重排序耗时
（`n` 为匹配数）。

## 测试

```sh
cargo test
```

## 许可证

MIT —— 见 [LICENSE](LICENSE)。
