//! Stable names: Rust v0 symbol names with every crate-root hash dropped.
//!
//! A v0 name embeds each crate's disambiguator (`Cs37q7ggDDgLf_9testcrate`),
//! which cargo derives from package id, version, path and features. Those
//! change on a version bump or a new checkout, so they can't key a slot.
//! The stable form keeps everything else: module path, item disambiguators
//! (`do_panic#1`), closures/shims, generic args, and the instantiating crate's
//! name (so two crates' copies of the same generic stay distinct).
//!
//! Non-`_R` names (C symbols like `memcpy`) are returned unchanged.

pub fn stable_name(sym: &str) -> Result<String, String> {
    let Some(rest) = sym.strip_prefix("_R") else {
        return Ok(sym.to_string());
    };
    // A vendor suffix (`.llvm.123`) is not part of the mangling.
    let (mangled, suffix) = match rest.find('.') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if mangled.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!("{sym}: v0 encoding version not supported"));
    }
    let mut p = Parser {
        s: mangled.as_bytes(),
        pos: 0,
        depth: 0,
    };
    let mut out = p.path().map_err(|e| format!("{sym}: {e}"))?;
    if p.pos < p.s.len() {
        // Instantiating crate: keep its name, drop its hash.
        let krate = p.path().map_err(|e| format!("{sym}: {e}"))?;
        out = format!("{out} @{krate}");
    }
    if p.pos != p.s.len() {
        return Err(format!("{sym}: trailing bytes at {}", p.pos));
    }
    Ok(out + suffix)
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    depth: u32,
}

type R<T> = Result<T, String>;

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }
    fn next(&mut self) -> R<u8> {
        let c = self.peek().ok_or("unexpected end")?;
        self.pos += 1;
        Ok(c)
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `_` = 0, `<digits>_` = value + 1.
    fn base62(&mut self) -> R<u64> {
        if self.eat(b'_') {
            return Ok(0);
        }
        let mut v: u64 = 0;
        loop {
            let c = self.next()?;
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'z' => c - b'a' + 10,
                b'A'..=b'Z' => c - b'A' + 36,
                b'_' => return v.checked_add(1).ok_or_else(|| "overflow".into()),
                _ => return Err(format!("bad base62 digit {:?}", c as char)),
            };
            v = v
                .checked_mul(62)
                .and_then(|v| v.checked_add(d as u64))
                .ok_or("overflow")?;
        }
    }

    /// `s<base62>`: absent = 0, `s_` = 1, ...
    fn disambiguator(&mut self) -> R<u64> {
        if self.eat(b's') {
            Ok(self.base62()? + 1)
        } else {
            Ok(0)
        }
    }

    fn decimal(&mut self) -> R<usize> {
        let start = self.pos;
        // `0` stands alone; anything else is a run of digits.
        if self.eat(b'0') {
            return Ok(0);
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.s[start..self.pos])
            .unwrap()
            .parse()
            .map_err(|_| "expected decimal".into())
    }

    /// `[u] <decimal> [_] <bytes>`
    fn ident(&mut self) -> R<String> {
        let puny = self.eat(b'u');
        let len = self.decimal()?;
        self.eat(b'_');
        let end = self
            .pos
            .checked_add(len)
            .filter(|&e| e <= self.s.len())
            .ok_or("identifier runs past end")?;
        let raw = std::str::from_utf8(&self.s[self.pos..end]).map_err(|_| "non-utf8 ident")?;
        self.pos = end;
        // Punycode stays encoded; the marker keeps it from colliding with ASCII names.
        Ok(if puny {
            format!("u:{raw}")
        } else {
            raw.to_string()
        })
    }

    fn backref<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let at = self.pos - 1;
        let target = self.base62()? as usize;
        if target >= at {
            return Err("forward backref".into());
        }
        let saved = self.pos;
        self.pos = target;
        let r = f(self);
        self.pos = saved;
        r
    }

    fn guard(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > 200 {
            Err("nesting too deep".into())
        } else {
            Ok(())
        }
    }

    fn path(&mut self) -> R<String> {
        self.guard()?;
        let r = match self.next()? {
            b'C' => {
                // Crate root: the disambiguator is the crate hash. Drop it.
                self.disambiguator()?;
                self.ident()
            }
            b'M' => {
                self.disambiguator()?;
                let _parent = self.path()?;
                Ok(format!("<{}>", self.ty()?))
            }
            b'X' => {
                self.disambiguator()?;
                let _parent = self.path()?;
                let t = self.ty()?;
                Ok(format!("<{t} as {}>", self.path()?))
            }
            b'Y' => {
                let t = self.ty()?;
                Ok(format!("<{t} as {}>", self.path()?))
            }
            b'N' => {
                let ns = self.next()?;
                let parent = self.path()?;
                let dis = self.disambiguator()?;
                let name = self.ident()?;
                let tag = if dis > 0 {
                    format!("#{dis}")
                } else {
                    String::new()
                };
                Ok(match ns {
                    b'C' => format!(
                        "{parent}::{{closure{tag}}}{}",
                        if name.is_empty() {
                            String::new()
                        } else {
                            format!(":{name}")
                        }
                    ),
                    b'S' => format!("{parent}::{{shim:{name}{tag}}}"),
                    b'a'..=b'z' => format!("{parent}::{name}{tag}"),
                    other => format!("{parent}::{{{}:{name}{tag}}}", other as char),
                })
            }
            b'I' => {
                let base = self.path()?;
                let mut args = Vec::new();
                while !self.eat(b'E') {
                    args.push(self.generic_arg()?);
                }
                Ok(format!("{base}::<{}>", args.join(", ")))
            }
            b'B' => self.backref(|p| p.path()),
            c => Err(format!("bad path tag {:?} at {}", c as char, self.pos - 1)),
        };
        self.depth -= 1;
        r
    }

    fn generic_arg(&mut self) -> R<String> {
        if self.eat(b'L') {
            return Ok(format!("'{}", self.base62()?));
        }
        if self.eat(b'K') {
            return self.konst();
        }
        self.ty()
    }

    fn konst(&mut self) -> R<String> {
        if self.eat(b'p') {
            return Ok("_".into());
        }
        if self.eat(b'B') {
            return self.backref(|p| p.konst());
        }
        let t = self.next()?;
        let neg = self.eat(b'n');
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            self.pos += 1;
        }
        let hex = std::str::from_utf8(&self.s[start..self.pos])
            .unwrap()
            .to_string();
        if !self.eat(b'_') {
            return Err(format!("const of type {:?} not supported", t as char));
        }
        let v = u128::from_str_radix(if hex.is_empty() { "0" } else { &hex }, 16)
            .map_err(|e| e.to_string())?;
        Ok(match t {
            b'b' => (v != 0).to_string(),
            b'c' => format!("{:?}", char::from_u32(v as u32).ok_or("bad char const")?),
            b'a' | b'h' | b'i' | b'j' | b'l' | b'm' | b'n' | b'o' | b's' | b't' | b'x' | b'y' => {
                format!("{}{v}", if neg { "-" } else { "" })
            }
            other => return Err(format!("const of type {:?} not supported", other as char)),
        })
    }

    fn binder(&mut self) -> R<String> {
        if self.eat(b'G') {
            Ok(format!("for<{}> ", self.base62()? + 1))
        } else {
            Ok(String::new())
        }
    }

    fn ty(&mut self) -> R<String> {
        self.guard()?;
        let basic = |c: u8| -> Option<&'static str> {
            Some(match c {
                b'a' => "i8",
                b'b' => "bool",
                b'c' => "char",
                b'd' => "f64",
                b'e' => "str",
                b'f' => "f32",
                b'h' => "u8",
                b'i' => "isize",
                b'j' => "usize",
                b'l' => "i32",
                b'm' => "u32",
                b'n' => "i128",
                b'o' => "u128",
                b's' => "i16",
                b't' => "u16",
                b'u' => "()",
                b'v' => "...",
                b'x' => "i64",
                b'y' => "u64",
                b'z' => "!",
                b'p' => "_",
                _ => return None,
            })
        };
        let c = self.peek().ok_or("unexpected end")?;
        let r = if let Some(b) = basic(c) {
            self.pos += 1;
            Ok(b.to_string())
        } else if matches!(c, b'C' | b'M' | b'X' | b'Y' | b'N' | b'I') {
            self.path()
        } else {
            self.pos += 1;
            match c {
                b'A' => {
                    let t = self.ty()?;
                    Ok(format!("[{t}; {}]", self.konst()?))
                }
                b'S' => Ok(format!("[{}]", self.ty()?)),
                b'R' | b'Q' => {
                    let lt = if self.eat(b'L') {
                        format!("'{} ", self.base62()?)
                    } else {
                        String::new()
                    };
                    let m = if c == b'Q' { "mut " } else { "" };
                    Ok(format!("&{lt}{m}{}", self.ty()?))
                }
                b'P' => Ok(format!("*const {}", self.ty()?)),
                b'O' => Ok(format!("*mut {}", self.ty()?)),
                b'F' => {
                    let b = self.binder()?;
                    let unsafe_ = if self.eat(b'U') { "unsafe " } else { "" };
                    let abi = if self.eat(b'K') {
                        if self.eat(b'C') {
                            "extern \"C\" ".to_string()
                        } else {
                            format!("extern \"{}\" ", self.ident()?.replace('_', "-"))
                        }
                    } else {
                        String::new()
                    };
                    let mut args = Vec::new();
                    while !self.eat(b'E') {
                        args.push(self.ty()?);
                    }
                    let ret = self.ty()?;
                    Ok(format!("{b}{unsafe_}{abi}fn({}) -> {ret}", args.join(", ")))
                }
                b'D' => {
                    let b = self.binder()?;
                    let mut traits = Vec::new();
                    while !self.eat(b'E') {
                        let mut t = self.path()?;
                        let mut assoc = Vec::new();
                        while self.eat(b'p') {
                            let name = self.ident()?;
                            assoc.push(format!("{name} = {}", self.ty()?));
                        }
                        if !assoc.is_empty() {
                            t = format!("{t}<{}>", assoc.join(", "));
                        }
                        traits.push(t);
                    }
                    if !self.eat(b'L') {
                        return Err("dyn without lifetime".into());
                    }
                    let lt = self.base62()?;
                    Ok(format!("dyn {b}{} + '{lt}", traits.join(" + ")))
                }
                b'T' => {
                    let mut ts = Vec::new();
                    while !self.eat(b'E') {
                        ts.push(self.ty()?);
                    }
                    Ok(format!("({})", ts.join(", ")))
                }
                b'B' => self.backref(|p| p.ty()),
                _ => Err(format!("bad type tag {:?} at {}", c as char, self.pos - 1)),
            }
        };
        self.depth -= 1;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::stable_name;

    #[test]
    fn drops_crate_hash_only() {
        assert_eq!(
            stable_name("_RNvCs37q7ggDDgLf_9testcrate7combine").unwrap(),
            "testcrate::combine"
        );
        assert_eq!(
            stable_name("_RNvCs5BXFcsRaI0R_6helper3mix").unwrap(),
            "helper::mix"
        );
        // Same item, different crate hash: same stable name.
        assert_eq!(
            stable_name("_RNvCsAAAA_6helper3mix").unwrap(),
            "helper::mix"
        );
        assert_eq!(stable_name("memcpy").unwrap(), "memcpy");
    }

    #[test]
    fn keeps_item_disambiguators() {
        // `Ns0_` on an item = disambiguator 1 (s_ is 1, s0_ is 2).
        assert_eq!(
            stable_name("_RNvNvCs1_4core8do_panic7runtime").unwrap(),
            "core::do_panic::runtime"
        );
        assert_eq!(
            stable_name("_RNvNvCs1_4cores0_8do_panic7runtime").unwrap(),
            "core::do_panic#2::runtime"
        );
        assert_ne!(
            stable_name("_RNvNvCs1_4cores_8do_panic7runtime").unwrap(),
            stable_name("_RNvNvCs1_4cores0_8do_panic7runtime").unwrap()
        );
    }

    #[test]
    fn generics_closures_backrefs() {
        // core::ptr::drop_in_place::<testcrate::Foo>, instantiated in testcrate.
        let n = stable_name("_RINvNtCs1_4core3ptr13drop_in_placeNtCs2_9testcrate3FooEB4_").unwrap();
        assert_eq!(n, "core::ptr::drop_in_place::<testcrate::Foo> @core");
        assert_eq!(
            stable_name("_RNCNvCs1_1a1f0B3_").unwrap(),
            "a::f::{closure} @a"
        );
        assert_eq!(
            stable_name("_RNvMNtCs1_4core3fmtNtB2_9Formatter3pad").unwrap(),
            "<core::fmt::Formatter>::pad"
        );
        // Two nested empty closure names back to back: "00" is two idents, not one.
        assert_eq!(
            stable_name("_RNCNCNvCs1_1a1f00B5_").unwrap(),
            "a::f::{closure}::{closure} @a"
        );
    }
}
