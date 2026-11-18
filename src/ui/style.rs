//! Colours and the two palettes. 256-colour indices only: they render the same
//! in Windows Terminal, conhost and every SSH client.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Default,
    Indexed(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    pub fg: Color,
    pub bold: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        fg: Color::Default,
        bold: false,
    };

    pub const fn fg(color: Color) -> Self {
        Self {
            fg: color,
            bold: false,
        }
    }

    pub const fn bold(color: Color) -> Self {
        Self {
            fg: color,
            bold: true,
        }
    }


}

/// Semantic colours, so themes only swap numbers.
#[derive(Clone, Copy)]
pub struct Palette {
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
    pub cpu: Color,
    pub memory: Color,
    pub disk: Color,
    pub net: Color,
    pub temp: Color,
    pub fan: Color,
    pub battery: Color,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            text: Color::Indexed(252),
            dim: Color::Indexed(245),
            faint: Color::Indexed(238),
            accent: Color::Indexed(45),
            ok: Color::Indexed(42),
            warn: Color::Indexed(214),
            bad: Color::Indexed(203),
            cpu: Color::Indexed(39),
            memory: Color::Indexed(75),
            disk: Color::Indexed(114),
            net: Color::Indexed(44),
            temp: Color::Indexed(208),
            fan: Color::Indexed(141),
            battery: Color::Indexed(40),
        }
    }

    pub fn light() -> Self {
        Self {
            text: Color::Indexed(235),
            dim: Color::Indexed(242),
            faint: Color::Indexed(250),
            accent: Color::Indexed(25),
            ok: Color::Indexed(28),
            warn: Color::Indexed(130),
            bad: Color::Indexed(124),
            cpu: Color::Indexed(25),
            memory: Color::Indexed(30),
            disk: Color::Indexed(64),
            net: Color::Indexed(24),
            temp: Color::Indexed(166),
            fan: Color::Indexed(90),
            battery: Color::Indexed(22),
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Load-based colour: calm until it matters.
    pub fn load(&self, percent: f32) -> Color {
        if percent >= 85.0 {
            self.bad
        } else if percent >= 60.0 {
            self.warn
        } else {
            self.ok
        }
    }

    pub fn temperature(&self, celsius: f32) -> Color {
        if celsius >= 90.0 {
            self.bad
        } else if celsius >= 75.0 {
            self.warn
        } else {
            self.ok
        }
    }
}

/// SGR escape for a style; `None` when the terminal default applies.
pub fn sgr(style: Style, out: &mut String) {
    out.push_str("\x1b[0");
    if style.bold {
        out.push_str(";1");
    }
    if let Color::Indexed(index) = style.fg {
        out.push_str(";38;5;");
        push_u16(out, index as u16);
    }
    out.push('m');
}

fn push_u16(out: &mut String, mut value: u16) {
    let mut digits = [0u8; 5];
    let mut len = 0;
    if value == 0 {
        out.push('0');
        return;
    }
    while value > 0 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for index in (0..len).rev() {
        out.push(digits[index] as char);
    }
}
