//! Color themes. Every visible color in the TUI comes from the active [`Theme`], so a
//! palette swap restyles panels, diffs, syntax and feedback together. The default theme keeps
//! Kiri's original colors byte for byte; the PTY smoke test depends on that.

use ratatui::style::Color;

/// Syntax token colors. Comments use the theme's muted color and plain variables use its
/// text color, so only the distinctive kinds are listed here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Syntax {
    pub keyword: Color,
    pub string: Color,
    pub number: Color,
    pub function: Color,
    pub type_: Color,
    pub property: Color,
    pub builtin: Color,
    pub namespace: Color,
    pub operator: Color,
    pub punctuation: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color,
    pub panel: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub border: Color,
    pub selected: Color,
    pub add: Color,
    pub remove: Color,
    pub add_bg: Color,
    pub remove_bg: Color,
    pub folder: Color,
    pub modified: Color,
    /// Background of the hunk header that keyboard hunk navigation is currently on.
    pub hunk: Color,
    /// Peak of the notice-bar flash after a Git mutation completes; it settles back to `bg`.
    pub pulse: Color,
    pub syntax: Syntax,
}

const fn hex(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Blend `percent` of `tint` into `base`. Non-RGB colors (the terminal-palette theme) are
/// returned unchanged because their real values are unknown here.
pub const fn mix(base: Color, tint: Color, percent: u8) -> Color {
    match (base, tint) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
            mix_channel(r1, r2, percent),
            mix_channel(g1, g2, percent),
            mix_channel(b1, b2, percent),
        ),
        _ => base,
    }
}

const fn mix_channel(base: u8, tint: u8, percent: u8) -> u8 {
    let base = base as i32;
    let tint = tint as i32;
    (base + (tint - base) * percent as i32 / 100) as u8
}

#[allow(clippy::too_many_arguments)]
const fn syn(
    keyword: u32,
    string: u32,
    number: u32,
    function: u32,
    type_: u32,
    property: u32,
    builtin: u32,
    namespace: u32,
    operator: u32,
    punctuation: u32,
) -> Syntax {
    Syntax {
        keyword: hex(keyword),
        string: hex(string),
        number: hex(number),
        function: hex(function),
        type_: hex(type_),
        property: hex(property),
        builtin: hex(builtin),
        namespace: hex(namespace),
        operator: hex(operator),
        punctuation: hex(punctuation),
    }
}

#[allow(clippy::too_many_arguments)]
const fn theme(
    id: &'static str,
    name: &'static str,
    dark: bool,
    bg: u32,
    panel: u32,
    text: u32,
    muted: u32,
    accent: u32,
    border: u32,
    selected: u32,
    add: u32,
    remove: u32,
    folder: u32,
    modified: u32,
    syntax: Syntax,
) -> Theme {
    let tint = if dark { 16 } else { 14 };
    Theme {
        id,
        name,
        dark,
        bg: hex(bg),
        panel: hex(panel),
        text: hex(text),
        // Theme source palettes often make comments too faint for terminal UI labels. Move the
        // muted tone halfway toward body text so it keeps its hue and remains readable.
        muted: mix(hex(muted), hex(text), 60),
        accent: hex(accent),
        border: hex(border),
        selected: hex(selected),
        add: hex(add),
        remove: hex(remove),
        add_bg: mix(hex(panel), hex(add), tint),
        remove_bg: mix(hex(panel), hex(remove), tint),
        folder: hex(folder),
        modified: hex(modified),
        hunk: mix(hex(selected), hex(accent), 18),
        pulse: mix(hex(bg), hex(accent), 22),
        syntax,
    }
}

/// Kiri's original palette, kept explicit so the default never drifts.
pub const KIRI: Theme = Theme {
    id: "kiri",
    name: "Kiri",
    dark: true,
    bg: Color::Rgb(15, 19, 24),
    panel: Color::Rgb(20, 25, 31),
    text: Color::Rgb(218, 224, 231),
    muted: Color::Rgb(139, 154, 172),
    accent: Color::Rgb(114, 215, 187),
    border: Color::Rgb(52, 69, 88),
    selected: Color::Rgb(30, 51, 70),
    add: Color::Rgb(140, 215, 166),
    remove: Color::Rgb(235, 155, 157),
    add_bg: Color::Rgb(22, 43, 35),
    remove_bg: Color::Rgb(47, 29, 35),
    folder: Color::Rgb(128, 179, 255),
    modified: Color::Rgb(235, 193, 112),
    hunk: Color::Rgb(37, 67, 66),
    pulse: Color::Rgb(27, 64, 55),
    syntax: Syntax {
        keyword: Color::Rgb(194, 152, 255),
        string: Color::Rgb(156, 219, 150),
        number: Color::Rgb(247, 174, 122),
        function: Color::Rgb(123, 185, 255),
        type_: Color::Rgb(241, 207, 127),
        property: Color::Rgb(114, 205, 226),
        builtin: Color::Rgb(228, 166, 232),
        namespace: Color::Rgb(174, 170, 249),
        operator: Color::Rgb(137, 212, 216),
        punctuation: Color::Rgb(172, 187, 204),
    },
};

/// Uses the terminal's own ANSI palette, so it follows whatever the emulator is configured
/// with. Diff line backgrounds and the completion flash are unavailable without RGB.
pub const TERMINAL: Theme = Theme {
    id: "terminal",
    name: "Terminal palette",
    dark: true,
    bg: Color::Reset,
    panel: Color::Reset,
    text: Color::Reset,
    muted: Color::DarkGray,
    accent: Color::Cyan,
    border: Color::DarkGray,
    selected: Color::DarkGray,
    add: Color::Green,
    remove: Color::Red,
    add_bg: Color::Reset,
    remove_bg: Color::Reset,
    folder: Color::Blue,
    modified: Color::Yellow,
    hunk: Color::DarkGray,
    pulse: Color::Reset,
    syntax: Syntax {
        keyword: Color::Magenta,
        string: Color::Green,
        number: Color::Yellow,
        function: Color::Blue,
        type_: Color::Yellow,
        property: Color::Cyan,
        builtin: Color::Magenta,
        namespace: Color::Blue,
        operator: Color::Cyan,
        punctuation: Color::Gray,
    },
};

pub const ALL: &[Theme] = &[
    KIRI,
    theme(
        "kiri-light",
        "Kiri Light",
        false,
        0xf6f8fb,
        0xffffff,
        0x1f2933,
        0x61708a,
        0x0f8f74,
        0xd3dbe6,
        0xdff3ec,
        0x1a7f4b,
        0xc03a45,
        0x2f6fd3,
        0xa5680c,
        syn(
            0x7a3ecf, 0x2a7f3f, 0xb35a1d, 0x1d5fc4, 0x9a6b00, 0x0f7a94, 0xa03fa8, 0x5b55c4,
            0x0f7b83, 0x55657a,
        ),
    ),
    theme(
        "kiri-midnight",
        "Kiri Midnight",
        true,
        0x0a0e1a,
        0x0f1424,
        0xd7dcf0,
        0x7f8aa8,
        0x8fa8ff,
        0x2a3352,
        0x1b2440,
        0x7fd6a3,
        0xf08f9b,
        0x6fb6ff,
        0xf0c674,
        syn(
            0xc7a2ff, 0x9bdc9a, 0xffb27a, 0x7fb8ff, 0xffd47f, 0x76d0e6, 0xec9ce9, 0xb0aefc,
            0x8fd6da, 0xa3afc7,
        ),
    ),
    theme(
        "kiri-ember",
        "Kiri Ember",
        true,
        0x17110f,
        0x1e1614,
        0xebdfd6,
        0x9a8880,
        0xff9f6e,
        0x4a3730,
        0x3a2822,
        0xa5d68f,
        0xff8c82,
        0xffc27a,
        0xf3b968,
        syn(
            0xff8f7a, 0xcbe089, 0xffb86c, 0xffc27a, 0xf6d08a, 0x8ed3c5, 0xe9a0d9, 0xd3b3ff,
            0x9ad9c6, 0xb8a49a,
        ),
    ),
    theme(
        "catppuccin-mocha",
        "Catppuccin Mocha",
        true,
        0x1e1e2e,
        0x181825,
        0xcdd6f4,
        0xa6adc8,
        0x89b4fa,
        0x45475a,
        0x313244,
        0xa6e3a1,
        0xf38ba8,
        0x89dceb,
        0xf9e2af,
        syn(
            0xcba6f7, 0xa6e3a1, 0xfab387, 0x89b4fa, 0xf9e2af, 0xb4befe, 0xf38ba8, 0xf5c2e7,
            0x94e2d5, 0x9399b2,
        ),
    ),
    theme(
        "catppuccin-macchiato",
        "Catppuccin Macchiato",
        true,
        0x24273a,
        0x1e2030,
        0xcad3f5,
        0xa5adcb,
        0x8aadf4,
        0x494d64,
        0x363a4f,
        0xa6da95,
        0xed8796,
        0x91d7e3,
        0xeed49f,
        syn(
            0xc6a0f6, 0xa6da95, 0xf5a97f, 0x8aadf4, 0xeed49f, 0xb7bdf8, 0xed8796, 0xf5bde6,
            0x8bd5ca, 0x939ab7,
        ),
    ),
    theme(
        "catppuccin-frappe",
        "Catppuccin Frappé",
        true,
        0x303446,
        0x292c3c,
        0xc6d0f5,
        0xa5adce,
        0x8caaee,
        0x51576d,
        0x414559,
        0xa6d189,
        0xe78284,
        0x99d1db,
        0xe5c890,
        syn(
            0xca9ee6, 0xa6d189, 0xef9f76, 0x8caaee, 0xe5c890, 0xbabbf1, 0xe78284, 0xf4b8e4,
            0x81c8be, 0x949cbb,
        ),
    ),
    theme(
        "catppuccin-latte",
        "Catppuccin Latte",
        false,
        0xeff1f5,
        0xe6e9ef,
        0x4c4f69,
        0x6c6f85,
        0x1e66f5,
        0xbcc0cc,
        0xccd0da,
        0x40a02b,
        0xd20f39,
        0x04a5e5,
        0xdf8e1d,
        syn(
            0x8839ef, 0x40a02b, 0xfe640b, 0x1e66f5, 0xdf8e1d, 0x7287fd, 0xd20f39, 0xea76cb,
            0x179299, 0x7c7f93,
        ),
    ),
    theme(
        "dracula",
        "Dracula",
        true,
        0x282a36,
        0x21222c,
        0xf8f8f2,
        0x6272a4,
        0xbd93f9,
        0x44475a,
        0x44475a,
        0x50fa7b,
        0xff5555,
        0x8be9fd,
        0xf1fa8c,
        syn(
            0xff79c6, 0xf1fa8c, 0xbd93f9, 0x50fa7b, 0x8be9fd, 0xffb86c, 0xbd93f9, 0x8be9fd,
            0xff79c6, 0xf8f8f2,
        ),
    ),
    theme(
        "nord",
        "Nord",
        true,
        0x2e3440,
        0x3b4252,
        0xd8dee9,
        0x7b88a1,
        0x88c0d0,
        0x4c566a,
        0x434c5e,
        0xa3be8c,
        0xbf616a,
        0x81a1c1,
        0xebcb8b,
        syn(
            0x81a1c1, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0xd8dee9, 0x5e81ac, 0x8fbcbb,
            0x81a1c1, 0xeceff4,
        ),
    ),
    theme(
        "gruvbox-dark",
        "Gruvbox Dark",
        true,
        0x282828,
        0x1d2021,
        0xebdbb2,
        0xa89984,
        0xfe8019,
        0x504945,
        0x3c3836,
        0xb8bb26,
        0xfb4934,
        0x83a598,
        0xfabd2f,
        syn(
            0xfb4934, 0xb8bb26, 0xd3869b, 0xfabd2f, 0xfabd2f, 0x83a598, 0xfe8019, 0x8ec07c,
            0x8ec07c, 0xa89984,
        ),
    ),
    theme(
        "gruvbox-light",
        "Gruvbox Light",
        false,
        0xfbf1c7,
        0xf9f5d7,
        0x3c3836,
        0x7c6f64,
        0xaf3a03,
        0xbdae93,
        0xebdbb2,
        0x79740e,
        0x9d0006,
        0x076678,
        0xb57614,
        syn(
            0x9d0006, 0x79740e, 0x8f3f71, 0xb57614, 0xb57614, 0x076678, 0xaf3a03, 0x427b58,
            0x427b58, 0x7c6f64,
        ),
    ),
    theme(
        "tokyo-night",
        "Tokyo Night",
        true,
        0x1a1b26,
        0x16161e,
        0xc0caf5,
        0x565f89,
        0x7aa2f7,
        0x3b4261,
        0x292e42,
        0x9ece6a,
        0xf7768e,
        0x7dcfff,
        0xe0af68,
        syn(
            0xbb9af7, 0x9ece6a, 0xff9e64, 0x7aa2f7, 0x2ac3de, 0x73daca, 0xf7768e, 0x7dcfff,
            0x89ddff, 0xa9b1d6,
        ),
    ),
    theme(
        "tokyo-night-storm",
        "Tokyo Night Storm",
        true,
        0x24283b,
        0x1f2335,
        0xc0caf5,
        0x565f89,
        0x7aa2f7,
        0x3b4261,
        0x2e3450,
        0x9ece6a,
        0xf7768e,
        0x7dcfff,
        0xe0af68,
        syn(
            0xbb9af7, 0x9ece6a, 0xff9e64, 0x7aa2f7, 0x2ac3de, 0x73daca, 0xf7768e, 0x7dcfff,
            0x89ddff, 0xa9b1d6,
        ),
    ),
    theme(
        "tokyo-night-day",
        "Tokyo Night Day",
        false,
        0xe1e2e7,
        0xd0d5e3,
        0x3760bf,
        0x848cb5,
        0x2e7de9,
        0xa8aecb,
        0xc4c8da,
        0x587539,
        0xf52a65,
        0x007197,
        0x8c6c3e,
        syn(
            0x9854f1, 0x587539, 0xb15c00, 0x2e7de9, 0x007197, 0x387068, 0xf52a65, 0x007197,
            0x006a83, 0x6172b0,
        ),
    ),
    theme(
        "one-dark",
        "One Dark",
        true,
        0x282c34,
        0x21252b,
        0xabb2bf,
        0x5c6370,
        0x61afef,
        0x3e4451,
        0x2c313c,
        0x98c379,
        0xe06c75,
        0x61afef,
        0xe5c07b,
        syn(
            0xc678dd, 0x98c379, 0xd19a66, 0x61afef, 0xe5c07b, 0xe06c75, 0x56b6c2, 0xe5c07b,
            0x56b6c2, 0xabb2bf,
        ),
    ),
    theme(
        "solarized-dark",
        "Solarized Dark",
        true,
        0x002b36,
        0x073642,
        0x93a1a1,
        0x657b83,
        0x268bd2,
        0x586e75,
        0x0f4a5a,
        0x859900,
        0xdc322f,
        0x268bd2,
        0xb58900,
        syn(
            0x859900, 0x2aa198, 0xd33682, 0x268bd2, 0xb58900, 0x839496, 0xcb4b16, 0x6c71c4,
            0x93a1a1, 0x657b83,
        ),
    ),
    theme(
        "solarized-light",
        "Solarized Light",
        false,
        0xfdf6e3,
        0xeee8d5,
        0x586e75,
        0x93a1a1,
        0x268bd2,
        0x93a1a1,
        0xe3dcc4,
        0x859900,
        0xdc322f,
        0x268bd2,
        0xb58900,
        syn(
            0x859900, 0x2aa198, 0xd33682, 0x268bd2, 0xb58900, 0x586e75, 0xcb4b16, 0x6c71c4,
            0x657b83, 0x839496,
        ),
    ),
    theme(
        "rose-pine",
        "Rosé Pine",
        true,
        0x191724,
        0x1f1d2e,
        0xe0def4,
        0x908caa,
        0xebbcba,
        0x403d52,
        0x26233a,
        0x9ccfd8,
        0xeb6f92,
        0xc4a7e7,
        0xf6c177,
        syn(
            0x31748f, 0xf6c177, 0xebbcba, 0xebbcba, 0x9ccfd8, 0xc4a7e7, 0xeb6f92, 0xc4a7e7,
            0x908caa, 0x6e6a86,
        ),
    ),
    theme(
        "rose-pine-moon",
        "Rosé Pine Moon",
        true,
        0x232136,
        0x2a273f,
        0xe0def4,
        0x908caa,
        0xea9a97,
        0x44415a,
        0x393552,
        0x9ccfd8,
        0xeb6f92,
        0xc4a7e7,
        0xf6c177,
        syn(
            0x3e8fb0, 0xf6c177, 0xea9a97, 0xea9a97, 0x9ccfd8, 0xc4a7e7, 0xeb6f92, 0xc4a7e7,
            0x908caa, 0x6e6a86,
        ),
    ),
    theme(
        "rose-pine-dawn",
        "Rosé Pine Dawn",
        false,
        0xfaf4ed,
        0xfffaf3,
        0x575279,
        0x797593,
        0xd7827e,
        0xdfdad9,
        0xf2e9e1,
        0x56949f,
        0xb4637a,
        0x907aa9,
        0xea9d34,
        syn(
            0x286983, 0xea9d34, 0xd7827e, 0xd7827e, 0x56949f, 0x907aa9, 0xb4637a, 0x907aa9,
            0x797593, 0x9893a5,
        ),
    ),
    theme(
        "monokai",
        "Monokai",
        true,
        0x272822,
        0x1e1f1c,
        0xf8f8f2,
        0x75715e,
        0xa6e22e,
        0x49483e,
        0x3e3d32,
        0xa6e22e,
        0xf92672,
        0x66d9ef,
        0xe6db74,
        syn(
            0xf92672, 0xe6db74, 0xae81ff, 0xa6e22e, 0x66d9ef, 0xf8f8f2, 0x66d9ef, 0xa6e22e,
            0xf92672, 0xf8f8f2,
        ),
    ),
    theme(
        "monokai-pro",
        "Monokai Pro",
        true,
        0x2d2a2e,
        0x221f22,
        0xfcfcfa,
        0x727072,
        0xffd866,
        0x5b595c,
        0x403e41,
        0xa9dc76,
        0xff6188,
        0x78dce8,
        0xffd866,
        syn(
            0xff6188, 0xffd866, 0xab9df2, 0xa9dc76, 0x78dce8, 0xfcfcfa, 0x78dce8, 0xa9dc76,
            0xff6188, 0x939293,
        ),
    ),
    theme(
        "everforest-dark",
        "Everforest Dark",
        true,
        0x2d353b,
        0x232a2e,
        0xd3c6aa,
        0x859289,
        0xa7c080,
        0x475258,
        0x3d484d,
        0xa7c080,
        0xe67e80,
        0x7fbbb3,
        0xdbbc7f,
        syn(
            0xe67e80, 0xdbbc7f, 0xd699b6, 0xa7c080, 0x83c092, 0xd3c6aa, 0xe69875, 0x7fbbb3,
            0xe69875, 0x9da9a0,
        ),
    ),
    theme(
        "everforest-light",
        "Everforest Light",
        false,
        0xfdf6e3,
        0xf4f0d9,
        0x5c6a72,
        0x939f91,
        0x8da101,
        0xe0dcc7,
        0xefebd4,
        0x8da101,
        0xf85552,
        0x3a94c5,
        0xdfa000,
        syn(
            0xf85552, 0xdfa000, 0xdf69ba, 0x8da101, 0x35a77c, 0x5c6a72, 0xf57d26, 0x3a94c5,
            0xf57d26, 0x829181,
        ),
    ),
    theme(
        "kanagawa",
        "Kanagawa",
        true,
        0x1f1f28,
        0x16161d,
        0xdcd7ba,
        0x727169,
        0x7e9cd8,
        0x54546d,
        0x2d4f67,
        0x98bb6c,
        0xe46876,
        0x7fb4ca,
        0xe6c384,
        syn(
            0x957fb8, 0x98bb6c, 0xd27e99, 0x7e9cd8, 0x7aa89f, 0xdcd7ba, 0xffa066, 0x7aa89f,
            0xc0a36e, 0x9cabca,
        ),
    ),
    theme(
        "ayu-dark",
        "Ayu Dark",
        true,
        0x0b0e14,
        0x0d1017,
        0xbfbdb6,
        0x565b66,
        0xe6b450,
        0x1c2028,
        0x1b2a3a,
        0xaad94c,
        0xf07178,
        0x59c2ff,
        0xffb454,
        syn(
            0xff8f40, 0xaad94c, 0xd2a6ff, 0xffb454, 0x59c2ff, 0xbfbdb6, 0x39bae6, 0x59c2ff,
            0xf29668, 0xbfbdb6,
        ),
    ),
    theme(
        "ayu-mirage",
        "Ayu Mirage",
        true,
        0x1f2430,
        0x1c212b,
        0xcccac2,
        0x707a8c,
        0xffcc66,
        0x2f3645,
        0x2c3547,
        0xd5ff80,
        0xf28779,
        0x73d0ff,
        0xffd173,
        syn(
            0xffad66, 0xd5ff80, 0xdfbfff, 0xffd173, 0x73d0ff, 0xcccac2, 0x5ccfe6, 0x73d0ff,
            0xf29e74, 0xcccac2,
        ),
    ),
    theme(
        "ayu-light",
        "Ayu Light",
        false,
        0xfcfcfc,
        0xf8f9fa,
        0x5c6166,
        0x8a9199,
        0xffaa33,
        0xe0e2e5,
        0xe8eef7,
        0x86b300,
        0xf07171,
        0x399ee6,
        0xf2ae49,
        syn(
            0xfa8d3e, 0x86b300, 0xa37acc, 0xf2ae49, 0x399ee6, 0x5c6166, 0x55b4d4, 0x399ee6,
            0xed9366, 0x5c6166,
        ),
    ),
    theme(
        "github-dark",
        "GitHub Dark",
        true,
        0x0d1117,
        0x161b22,
        0xc9d1d9,
        0x8b949e,
        0x58a6ff,
        0x30363d,
        0x1c2a3f,
        0x3fb950,
        0xf85149,
        0x79c0ff,
        0xd29922,
        syn(
            0xff7b72, 0xa5d6ff, 0x79c0ff, 0xd2a8ff, 0xffa657, 0x79c0ff, 0xffa657, 0xff7b72,
            0xc9d1d9, 0x8b949e,
        ),
    ),
    theme(
        "github-light",
        "GitHub Light",
        false,
        0xffffff,
        0xf6f8fa,
        0x24292f,
        0x57606a,
        0x0969da,
        0xd0d7de,
        0xddf4ff,
        0x1a7f37,
        0xcf222e,
        0x0969da,
        0x9a6700,
        syn(
            0xcf222e, 0x0a3069, 0x0550ae, 0x8250df, 0x953800, 0x0550ae, 0x953800, 0xcf222e,
            0x24292f, 0x57606a,
        ),
    ),
    theme(
        "palenight",
        "Material Palenight",
        true,
        0x292d3e,
        0x232637,
        0xa6accd,
        0x676e95,
        0x82aaff,
        0x3f4460,
        0x34324a,
        0xc3e88d,
        0xf07178,
        0x82aaff,
        0xffcb6b,
        syn(
            0xc792ea, 0xc3e88d, 0xf78c6c, 0x82aaff, 0xffcb6b, 0xa6accd, 0x89ddff, 0xffcb6b,
            0x89ddff, 0x89ddff,
        ),
    ),
    theme(
        "material-ocean",
        "Material Ocean",
        true,
        0x0f111a,
        0x090b10,
        0xa6accd,
        0x464b5d,
        0x84ffff,
        0x1f2233,
        0x1f2233,
        0xc3e88d,
        0xf07178,
        0x82aaff,
        0xffcb6b,
        syn(
            0xc792ea, 0xc3e88d, 0xf78c6c, 0x82aaff, 0xffcb6b, 0xa6accd, 0x89ddff, 0xffcb6b,
            0x89ddff, 0x89ddff,
        ),
    ),
    theme(
        "night-owl",
        "Night Owl",
        true,
        0x011627,
        0x01111d,
        0xd6deeb,
        0x637777,
        0x82aaff,
        0x1d3b53,
        0x0b2942,
        0xaddb67,
        0xef5350,
        0x82aaff,
        0xecc48d,
        syn(
            0xc792ea, 0xecc48d, 0xf78c6c, 0x82aaff, 0xffcb8b, 0x7fdbca, 0x7fdbca, 0xffcb8b,
            0x7fdbca, 0xd6deeb,
        ),
    ),
    theme(
        "nightfly",
        "Nightfly",
        true,
        0x011627,
        0x021c33,
        0xc3ccdc,
        0x7c8f8f,
        0x82aaff,
        0x1d3b53,
        0x13324f,
        0xa1cd5e,
        0xfc514e,
        0x82aaff,
        0xe3d18a,
        syn(
            0xc792ea, 0xecc48d, 0xf78c6c, 0x82aaff, 0xffcb8b, 0x7fdbca, 0x7fdbca, 0xffcb8b,
            0x7fdbca, 0xc3ccdc,
        ),
    ),
    theme(
        "synthwave-84",
        "SynthWave '84",
        true,
        0x262335,
        0x241b2f,
        0xf4eee4,
        0x848bbd,
        0xff7edb,
        0x495495,
        0x34294f,
        0x72f1b8,
        0xfe4450,
        0x36f9f6,
        0xfede5d,
        syn(
            0xfede5d, 0xff8b39, 0xf97e72, 0x36f9f6, 0xfe4450, 0xff7edb, 0xff7edb, 0x36f9f6,
            0xff7edb, 0xbbbbbb,
        ),
    ),
    theme(
        "nightfox",
        "Nightfox",
        true,
        0x192330,
        0x131a24,
        0xcdcecf,
        0x738091,
        0x719cd6,
        0x29394f,
        0x2b3b51,
        0x81b29a,
        0xc94f6d,
        0x63cdcf,
        0xdbc074,
        syn(
            0x9d79d6, 0x81b29a, 0xf4a261, 0x719cd6, 0xdbc074, 0x63cdcf, 0xc94f6d, 0xd67ad2,
            0x63cdcf, 0xaeafb0,
        ),
    ),
    theme(
        "zenburn",
        "Zenburn",
        true,
        0x3f3f3f,
        0x383838,
        0xdcdccc,
        0x7f9f7f,
        0xf0dfaf,
        0x5f5f5f,
        0x4f4f4f,
        0x7f9f7f,
        0xcc9393,
        0x8cd0d3,
        0xf0dfaf,
        syn(
            0xf0dfaf, 0xcc9393, 0x8cd0d3, 0xefef8f, 0xdfdfbf, 0xdcdccc, 0xdc8cc3, 0xdfaf8f,
            0xf0efd0, 0xdcdccc,
        ),
    ),
    theme(
        "cobalt2",
        "Cobalt2",
        true,
        0x193549,
        0x122738,
        0xffffff,
        0x5c8fb8,
        0xffc600,
        0x1f4662,
        0x0d3a58,
        0x3ad900,
        0xff628c,
        0x9effff,
        0xffc600,
        syn(
            0xff9d00, 0xa5ff90, 0xff628c, 0xffc600, 0x80ffbb, 0xffffff, 0xff628c, 0x9effff,
            0xff9d00, 0xffffff,
        ),
    ),
    theme(
        "horizon",
        "Horizon",
        true,
        0x1c1e26,
        0x232530,
        0xd5d8da,
        0x6c6f93,
        0xe95678,
        0x2e303e,
        0x2e303e,
        0x29d398,
        0xe95678,
        0x26bbd9,
        0xfab795,
        syn(
            0xb877db, 0xfab795, 0xf09383, 0x25b0bc, 0xfab795, 0xd5d8da, 0xe95678, 0x59e1e3,
            0x6c6f93, 0xd5d8da,
        ),
    ),
    theme(
        "oxocarbon",
        "Oxocarbon",
        true,
        0x161616,
        0x262626,
        0xf2f4f8,
        0x525252,
        0x3ddbd9,
        0x393939,
        0x393939,
        0x42be65,
        0xee5396,
        0x78a9ff,
        0x33b1ff,
        syn(
            0xbe95ff, 0x42be65, 0x3ddbd9, 0x33b1ff, 0x08bdba, 0xff7eb6, 0xee5396, 0x78a9ff,
            0x82cfff, 0xdde1e6,
        ),
    ),
    theme(
        "vesper",
        "Vesper",
        true,
        0x101010,
        0x161616,
        0xffffff,
        0x8b8b8b,
        0xffc799,
        0x282828,
        0x232323,
        0x99ffe4,
        0xff8080,
        0xffc799,
        0xffc799,
        syn(
            0xa0a0a0, 0x99ffe4, 0xffc799, 0xffc799, 0xffc799, 0xffffff, 0xffc799, 0xa0a0a0,
            0xa0a0a0, 0xa0a0a0,
        ),
    ),
    theme(
        "poimandres",
        "Poimandres",
        true,
        0x1b1e28,
        0x171922,
        0xa6accd,
        0x767c9d,
        0x5de4c7,
        0x303340,
        0x303340,
        0x5de4c7,
        0xd0679d,
        0xadd7ff,
        0xfffac2,
        syn(
            0x91b4d5, 0x5de4c7, 0x5fb3a1, 0xadd7ff, 0xe4f0fb, 0xa6accd, 0xd0679d, 0x89ddff,
            0x91b4d5, 0xa6accd,
        ),
    ),
    theme(
        "moonlight",
        "Moonlight",
        true,
        0x222436,
        0x1e2030,
        0xc8d3f5,
        0x828bb8,
        0x82aaff,
        0x2f334d,
        0x2d3f76,
        0xc3e88d,
        0xff757f,
        0x86e1fc,
        0xffc777,
        syn(
            0xc099ff, 0xc3e88d, 0xff966c, 0x82aaff, 0xffc777, 0xc8d3f5, 0x86e1fc, 0xffc777,
            0x86e1fc, 0x828bb8,
        ),
    ),
    theme(
        "andromeda",
        "Andromeda",
        true,
        0x23262e,
        0x1e2025,
        0xd5ced9,
        0x746f77,
        0x00e8c6,
        0x2b303b,
        0x2f333d,
        0x96e072,
        0xee5d43,
        0x7cb7ff,
        0xffe66d,
        syn(
            0xc74ded, 0x96e072, 0xee5d43, 0xffe66d, 0xffe66d, 0xd5ced9, 0x00e8c6, 0xff00aa,
            0x00e8c6, 0xd5ced9,
        ),
    ),
    theme(
        "shades-of-purple",
        "Shades of Purple",
        true,
        0x2d2b55,
        0x1e1e3f,
        0xe3dfff,
        0xa599e9,
        0xfad000,
        0x4d4b7c,
        0x3e3a7c,
        0x3ad900,
        0xec3a37,
        0x9effff,
        0xfad000,
        syn(
            0xff9d00, 0xa5ff90, 0xff628c, 0xfad000, 0xfb94ff, 0xe3dfff, 0xff628c, 0x9effff,
            0xff9d00, 0xe3dfff,
        ),
    ),
    theme(
        "vitesse-dark",
        "Vitesse Dark",
        true,
        0x121212,
        0x181818,
        0xdbd7ca,
        0x758575,
        0x4d9375,
        0x2a2a2a,
        0x222222,
        0x4d9375,
        0xcb7676,
        0x6394bf,
        0xe6cc77,
        syn(
            0xcb7676, 0xc98a7d, 0x4c9a91, 0x80a665, 0x5da994, 0xb8a965, 0xbd976a, 0x5da994,
            0xcb7676, 0x666666,
        ),
    ),
    theme(
        "vitesse-light",
        "Vitesse Light",
        false,
        0xffffff,
        0xf7f7f7,
        0x393a34,
        0xa0ada0,
        0x1c6b48,
        0xe6e6e6,
        0xeaeaea,
        0x1c6b48,
        0xab5959,
        0x296aa3,
        0xbda437,
        syn(
            0xab5959, 0xb56959, 0x2f798a, 0x59873a, 0x2e8f82, 0x998418, 0xb58451, 0x2e8f82,
            0xab5959, 0x999999,
        ),
    ),
    theme(
        "flexoki-dark",
        "Flexoki Dark",
        true,
        0x100f0f,
        0x1c1b1a,
        0xcecdc3,
        0x878580,
        0xda702c,
        0x343331,
        0x282726,
        0x879a39,
        0xd14d41,
        0x4385be,
        0xd0a215,
        syn(
            0x8b7ec8, 0x3aa99f, 0x8b7ec8, 0xda702c, 0xd0a215, 0x4385be, 0xce5d97, 0x4385be,
            0x878580, 0x878580,
        ),
    ),
    theme(
        "flexoki-light",
        "Flexoki Light",
        false,
        0xfffcf0,
        0xf2f0e5,
        0x100f0f,
        0x6f6e69,
        0xbc5215,
        0xdad8ce,
        0xe6e4d9,
        0x66800b,
        0xaf3029,
        0x205ea6,
        0xad8301,
        syn(
            0x5e409d, 0x24837b, 0x5e409d, 0xbc5215, 0xad8301, 0x205ea6, 0xa02f6f, 0x205ea6,
            0x6f6e69, 0x6f6e69,
        ),
    ),
    theme(
        "modus-vivendi",
        "Modus Vivendi",
        true,
        0x000000,
        0x1e1e1e,
        0xffffff,
        0x989898,
        0x2fafff,
        0x303030,
        0x2f2f2f,
        0x44bc44,
        0xff5f59,
        0x79a8ff,
        0xd0bc00,
        syn(
            0xb6a0ff, 0x79a8ff, 0x00bcff, 0xfeacd0, 0x6ae4b9, 0x00d3d0, 0xf78fe7, 0x6ae4b9,
            0x989898, 0x989898,
        ),
    ),
    theme(
        "modus-operandi",
        "Modus Operandi",
        false,
        0xffffff,
        0xf0f0f0,
        0x000000,
        0x595959,
        0x0031a9,
        0xc4c4c4,
        0xc0deff,
        0x006800,
        0xa60000,
        0x3548cf,
        0x6f5500,
        syn(
            0x5317ac, 0x3548cf, 0x0000c0, 0x721045, 0x005f5f, 0x005e8b, 0x8f0075, 0x005f5f,
            0x595959, 0x595959,
        ),
    ),
    theme(
        "iceberg",
        "Iceberg",
        true,
        0x161821,
        0x1e2132,
        0xc6c8d1,
        0x6b7089,
        0x84a0c6,
        0x3d425b,
        0x272c42,
        0xb4be82,
        0xe27878,
        0x84a0c6,
        0xe2a478,
        syn(
            0x84a0c6, 0x89b8c2, 0xa093c7, 0xa3adcb, 0xa093c7, 0xc6c8d1, 0x84a0c6, 0x89b8c2,
            0x84a0c6, 0xc6c8d1,
        ),
    ),
    theme(
        "sonokai",
        "Sonokai",
        true,
        0x2c2e34,
        0x23252b,
        0xe2e2e3,
        0x7f8490,
        0x9ed072,
        0x414550,
        0x3b3e48,
        0x9ed072,
        0xfc5d7c,
        0x76cce0,
        0xe7c664,
        syn(
            0xfc5d7c, 0xe7c664, 0xb39df3, 0x9ed072, 0x76cce0, 0xe2e2e3, 0xf39660, 0x76cce0,
            0xf39660, 0x7f8490,
        ),
    ),
    theme(
        "snazzy",
        "Snazzy",
        true,
        0x282a36,
        0x1e1f29,
        0xeff0eb,
        0x686868,
        0x57c7ff,
        0x3a3d4d,
        0x3a3d4d,
        0x5af78e,
        0xff5c57,
        0x57c7ff,
        0xf3f99d,
        syn(
            0xff6ac1, 0xf3f99d, 0x9aedfe, 0x57c7ff, 0x5af78e, 0xeff0eb, 0x57c7ff, 0x9aedfe,
            0xff6ac1, 0xeff0eb,
        ),
    ),
    theme(
        "papercolor-dark",
        "PaperColor Dark",
        true,
        0x1c1c1c,
        0x262626,
        0xd0d0d0,
        0x808080,
        0x5fafd7,
        0x3a3a3a,
        0x303030,
        0xafd700,
        0xd7005f,
        0x5fafd7,
        0xffaf00,
        syn(
            0xaf87d7, 0xafd700, 0xff5faf, 0x5fafd7, 0xffaf00, 0xd0d0d0, 0x00afaf, 0x5fafd7,
            0xd0d0d0, 0x808080,
        ),
    ),
    theme(
        "papercolor-light",
        "PaperColor Light",
        false,
        0xeeeeee,
        0xe4e4e4,
        0x444444,
        0x878787,
        0x005f87,
        0xbcbcbc,
        0xd0d0d0,
        0x5f8700,
        0xaf0000,
        0x0087af,
        0xd75f00,
        syn(
            0x8700af, 0x5f8700, 0xd70087, 0x005f87, 0xd75f00, 0x444444, 0x0087af, 0x005f87,
            0x444444, 0x878787,
        ),
    ),
    theme(
        "tomorrow-night",
        "Tomorrow Night",
        true,
        0x1d1f21,
        0x282a2e,
        0xc5c8c6,
        0x969896,
        0x81a2be,
        0x373b41,
        0x373b41,
        0xb5bd68,
        0xcc6666,
        0x81a2be,
        0xf0c674,
        syn(
            0xb294bb, 0xb5bd68, 0xde935f, 0x81a2be, 0xf0c674, 0xc5c8c6, 0x8abeb7, 0xf0c674,
            0x8abeb7, 0xc5c8c6,
        ),
    ),
    theme(
        "tomorrow",
        "Tomorrow",
        false,
        0xffffff,
        0xefefef,
        0x4d4d4c,
        0x8e908c,
        0x4271ae,
        0xd6d6d6,
        0xe5e5e5,
        0x718c00,
        0xc82829,
        0x4271ae,
        0xeab700,
        syn(
            0x8959a8, 0x718c00, 0xf5871f, 0x4271ae, 0xeab700, 0x4d4d4c, 0x3e999f, 0xeab700,
            0x3e999f, 0x4d4d4c,
        ),
    ),
    theme(
        "base16-dark",
        "Base16 Default Dark",
        true,
        0x181818,
        0x282828,
        0xd8d8d8,
        0x8a8a8a,
        0x7cafc2,
        0x383838,
        0x383838,
        0xa1b56c,
        0xab4642,
        0x7cafc2,
        0xf7ca88,
        syn(
            0xba8baf, 0xa1b56c, 0xdc9656, 0x7cafc2, 0xf7ca88, 0xd8d8d8, 0x86c1b9, 0xf7ca88,
            0x86c1b9, 0xb8b8b8,
        ),
    ),
    theme(
        "darcula",
        "Darcula",
        true,
        0x2b2b2b,
        0x313335,
        0xa9b7c6,
        0x808080,
        0xffc66d,
        0x555555,
        0x214283,
        0x6a8759,
        0xff6b68,
        0x6897bb,
        0xffc66d,
        syn(
            0xcc7832, 0x6a8759, 0x6897bb, 0xffc66d, 0xa9b7c6, 0x9876aa, 0xcc7832, 0xa9b7c6,
            0xa9b7c6, 0xa9b7c6,
        ),
    ),
    TERMINAL,
];

impl Default for Theme {
    fn default() -> Self {
        KIRI
    }
}

impl Theme {
    pub fn all() -> &'static [Theme] {
        ALL
    }

    /// Look a theme up by identifier or display name, ignoring case, spaces and underscores.
    pub fn find(query: &str) -> Option<&'static Theme> {
        let wanted = normalize(query);
        if wanted.is_empty() {
            return None;
        }
        ALL.iter()
            .find(|theme| normalize(theme.id) == wanted || normalize(theme.name) == wanted)
    }

    pub fn position(id: &str) -> usize {
        ALL.iter().position(|theme| theme.id == id).unwrap_or(0)
    }

    /// Cycle to the next theme in the list, wrapping around.
    pub fn next(&self, step: isize) -> &'static Theme {
        let index = Theme::position(self.id) as isize + step;
        &ALL[index.rem_euclid(ALL.len() as isize) as usize]
    }

    pub fn ids() -> impl Iterator<Item = &'static str> {
        ALL.iter().map(|theme| theme.id)
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-' | '\''))
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Borders {
    #[default]
    Rounded,
    Plain,
    Double,
    Thick,
}

impl Borders {
    pub const ALL: [Self; 4] = [Self::Rounded, Self::Plain, Self::Double, Self::Thick];
    pub fn id(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Plain => "plain",
            Self::Double => "double",
            Self::Thick => "thick",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == value)
    }
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|b| *b == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
    pub fn border_type(self) -> ratatui::widgets::BorderType {
        use ratatui::widgets::BorderType;
        match self {
            Self::Rounded => BorderType::Rounded,
            Self::Plain => BorderType::Plain,
            Self::Double => BorderType::Double,
            Self::Thick => BorderType::Thick,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn luminance(color: Color) -> Option<f32> {
        match color {
            Color::Rgb(r, g, b) => {
                let linear = |channel: u8| {
                    let value = channel as f32 / 255.0;
                    if value <= 0.04045 {
                        value / 12.92
                    } else {
                        ((value + 0.055) / 1.055).powf(2.4)
                    }
                };
                Some(0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b))
            }
            _ => None,
        }
    }

    fn contrast(left: f32, right: f32) -> f32 {
        let (light, dark) = if left > right {
            (left, right)
        } else {
            (right, left)
        };
        (light + 0.05) / (dark + 0.05)
    }

    #[test]
    fn identifiers_are_unique_and_resolvable() {
        let mut seen = HashSet::new();
        for theme in ALL {
            assert!(seen.insert(theme.id), "duplicate theme id {}", theme.id);
            assert!(
                theme
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
            assert_eq!(Theme::find(theme.id).map(|t| t.id), Some(theme.id));
            assert_eq!(Theme::find(theme.name).map(|t| t.id), Some(theme.id));
        }
        assert!(ALL.len() >= 60);
        assert_eq!(
            Theme::find("Catppuccin_Mocha").map(|t| t.id),
            Some("catppuccin-mocha")
        );
        assert!(Theme::find("").is_none());
        assert!(Theme::find("not-a-theme").is_none());
    }

    #[test]
    fn default_theme_keeps_the_original_palette() {
        let theme = Theme::default();
        assert_eq!(theme.id, "kiri");
        assert_eq!(theme.bg, Color::Rgb(15, 19, 24));
        assert_eq!(theme.accent, Color::Rgb(114, 215, 187));
        assert_eq!(theme.syntax.keyword, Color::Rgb(194, 152, 255));
        assert_eq!(theme.syntax.property, Color::Rgb(114, 205, 226));
        assert_eq!(theme.pulse, Color::Rgb(27, 64, 55));
    }

    #[test]
    fn every_rgb_theme_keeps_text_readable_and_diff_tints_subtle() {
        for theme in ALL {
            let (Some(bg), Some(text), Some(panel), Some(muted)) = (
                luminance(theme.bg),
                luminance(theme.text),
                luminance(theme.panel),
                luminance(theme.muted),
            ) else {
                continue;
            };
            assert!(
                contrast(bg, text) >= 4.5,
                "{}: text contrast too low",
                theme.id
            );
            assert!(
                contrast(panel, muted) >= 3.0,
                "{}: muted text unreadable",
                theme.id
            );
            assert_eq!(theme.dark, bg < 0.5, "{}: dark flag mismatch", theme.id);
            for (name, tint) in [("add", theme.add_bg), ("remove", theme.remove_bg)] {
                let tint = luminance(tint).unwrap_or(panel);
                assert!(
                    contrast(tint, panel) < 1.8,
                    "{}: {name} background too loud",
                    theme.id
                );
            }
        }
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        let first = &ALL[0];
        assert_eq!(first.next(-1).id, ALL[ALL.len() - 1].id);
        assert_eq!(ALL[ALL.len() - 1].next(1).id, first.id);
        assert_eq!(Borders::Thick.next(), Borders::Rounded);
        assert_eq!(Borders::parse("double"), Some(Borders::Double));
    }
}
