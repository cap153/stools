//! Lightweight locale detection plus the bilingual default config templates.
//!
//! Only two things are localized today: the generated `config.toml` comments and
//! (on Windows) the tray menu labels. Every option name, value and structure
//! stays identical across languages, so a config written under one locale loads
//! fine under the other.

/// The English template lives in `config.rs`; re-exported here so callers and
/// tests can talk about both languages through one module.
pub use crate::core::config::DEFAULT_CONFIG_TEMPLATE as DEFAULT_CONFIG_TEMPLATE_EN;

/// Whether the current system language is Chinese (zh-CN, zh-TW, zh-HK, …).
pub fn is_chinese_locale() -> bool {
    #[cfg(windows)]
    {
        // Win32 answer first: LANGID primary language 0x04 is Chinese.
        unsafe {
            let lang_id = windows_sys::Win32::Globalization::GetUserDefaultUILanguage();
            if (lang_id & 0x03FF) == 0x0004 {
                return true;
            }
        }
    }

    // Standard POSIX vars (used on Linux, and as a Windows fallback).
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = std::env::var(var) {
            let lower = val.to_ascii_lowercase();
            if lower.starts_with("zh") || lower.contains("zh_") || lower.contains("zh-") {
                return true;
            }
        }
    }

    false
}

/// Chinese default config template. Same keys, same values, same structure.
pub const DEFAULT_CONFIG_TEMPLATE_ZH: &str = r##"# stools 启动器配置文件
#
# Linux   : ~/.config/stools/config.toml
# Windows : %APPDATA%\stools\config.toml
#
# 本文件在首次运行时自动生成，其中每一项都是可选的。删除本文件（或其中任意
# 一项配置）即可恢复软件的出厂行为。

# 额外检索目录（在系统内置路径之外追加）。所有目录都会检索可执行文件；在 Linux
# 下放置在这些目录中的 .desktop 文件也会被索引。支持 "~"、"$VAR"、"${VAR}" 与
# "%VAR%" 展开，不存在的目录会被忽略。
#
# Windows 路径提示：在双引号 "..." 内反斜杠会被当作 TOML 转义符，因此
# "C:\Users\me" 会在语法上出错。书写 Windows 路径时请使用单引号、正斜杠，或者
# 双反斜杠（三者均可被正确识别）：
#     'C:\Users\me\Downloads'      "C:/Users/me/Downloads"      "C:\\Users\\me"
path = [
#     "$HOME/.cargo/bin",
#     'C:\ProgramData\Microsoft\Windows\Start Menu\Programs',
#     '~/Downloads',
#     "C:/Tools",
]

# 快捷键：覆盖默认值或新增绑定。
#
# 动作 (Action)：
#   "down"    - 选择下一项
#   "up"      - 选择上一项
#   "execute" - 启动选中的条目
#   "close"   - Linux 下退出 stools / Windows 下隐藏窗口
#   "stools"  - 唤出窗口（在 Windows 上注册为全局热键；该动作只能绑定一次，
#               若把唤出热键移到别处，请删除此行）
#
# 按键名称不区分大小写，同时接受 XKB 拼写（Escape、Return、Prior、Next、space
# 等）以及常用别名（esc、enter、pageup、pagedown 等）。单个字符即表示对应的
# 实体按键（"a"、"u"、"/"）。
#
# 表名即修饰键的组合；修饰键可以自由组合、顺序不限：
#   [keybindings]                 - 无修饰键
#   [keybindings.none]            - 同上
#   [keybindings.alt_shift]
#   [keybindings."super+shift"]   - 使用 "+" 时需要对表名加引号
# 可识别的修饰键：ctrl (control)、alt (option)、shift、super (win、meta、cmd)。
[keybindings]
tab = "down"       # 选择下一项
esc = "close"      # Linux 下退出 stools / Windows 下隐藏窗口
Return = "execute" # 启动选中的条目
Up = "up"
Down = "down"

[keybindings.shift]
tab = "up"         # 选择上一项

[keybindings.alt]
a = "stools"       # 唤出窗口（Windows 全局热键）

# [keybindings.ctrl]
# u = "up"         # 示例：绑定 Ctrl+U 选择上一项
# e = "down"       # 示例：绑定 Ctrl+E 选择下一项
# 主题设置。颜色采用 Fuzzel 的 RRGGBBAA 十六进制记法（允许以 '#' 开头；RGB /
# RGBA / RRGGBB 同样支持），因此可以直接复用 Fuzzel 的主题配置。
# 下方为默认值（Dracula 配色）。
[theme]
# "match" / "selection-match" 用于高亮条目名称中与查询匹配的字符（选中行时使用
# selection-match），"prompt" 为输入框前 ">" 提示符的颜色。
background = "282a36dd"
text = "f8f8f2ff"
prompt = "586e75ff"
match = "8be9fdff"
selection-match = "8be9fdff"
selection = "44475add"
selection-text = "f8f8f2ff"
border = "bd93f9ff"

# 名称/路径超长时单次完整跑马灯循环所需时间。
# 数值越大滚动越慢，例如 "8s"（默认）、"12s"、"6500ms" 等。限制范围为 1s..60s。
marquee-duration = "8s"

# 字体优先级列表：按顺序选用系统中第一个已安装的字体族，因此一份列表即可同时覆盖
# 拉丁字符与中日韩文字。该字体缺失的字形会通过系统字体回退链自动补全。
font = [
    "ComicShannsMono Nerd Font",
    "LXGW WenKai GB Screen",
    "JetBrains Mono",
]
"##;

/// Pick the initial template according to the host system locale.
pub fn default_config_template() -> &'static str {
    if is_chinese_locale() {
        DEFAULT_CONFIG_TEMPLATE_ZH
    } else {
        DEFAULT_CONFIG_TEMPLATE_EN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;

    #[test]
    fn both_templates_parse_cleanly() {
        let en: Config =
            toml::from_str(DEFAULT_CONFIG_TEMPLATE_EN).expect("EN template parses");
        let zh: Config =
            toml::from_str(DEFAULT_CONFIG_TEMPLATE_ZH).expect("ZH template parses");
        assert_eq!(en.theme.background, zh.theme.background);
        assert_eq!(en.theme.prompt, zh.theme.prompt);
        assert_eq!(en.theme.marquee_duration, zh.theme.marquee_duration);
        assert_eq!(en.theme.font, zh.theme.font);
    }
}
