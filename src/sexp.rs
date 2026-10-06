//! Minimal S-expression reader for the VIR text log.
//!
//! The log is a pretty-print of Verus's AST: lists, atoms, and quoted strings
//! (spans). Nodes borrow from the input text.

#[derive(Debug)]
pub enum Node<'a> {
    Atom(&'a str),
    Str(&'a str),
    List(Vec<Node<'a>>),
}

impl<'a> Node<'a> {
    pub fn atom(&self) -> Option<&'a str> {
        match self {
            Node::Atom(a) => Some(a),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Node<'a>]> {
        match self {
            Node::List(v) => Some(v),
            _ => None,
        }
    }
    pub fn head(&self) -> Option<&'a str> {
        self.list().and_then(|v| v.first()).and_then(|n| n.atom())
    }
}

pub struct Reader<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    pub fn new(s: &'a str) -> Self {
        Reader {
            s,
            b: s.as_bytes(),
            i: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\n' | b'\t' | b'\r' => self.i += 1,
                b';' => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1
                    }
                }
                _ => break,
            }
        }
    }

    /// Next top-level form, `Ok(None)` at end of input.
    pub fn next_form(&mut self) -> Result<Option<Node<'a>>, String> {
        self.skip_ws();
        if self.i >= self.b.len() {
            return Ok(None);
        }
        self.node().map(Some)
    }

    fn node(&mut self) -> Result<Node<'a>, String> {
        self.skip_ws();
        let Some(&c) = self.b.get(self.i) else {
            return Err(format!("unexpected end of input at byte {}", self.i));
        };
        match c {
            b'(' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.skip_ws();
                    match self.b.get(self.i) {
                        None => return Err(format!("unterminated list at byte {}", self.i)),
                        Some(b')') => {
                            self.i += 1;
                            return Ok(Node::List(v));
                        }
                        Some(_) => v.push(self.node()?),
                    }
                }
            }
            b')' => Err(format!("unbalanced ')' at byte {}", self.i)),
            b'"' => {
                let st = self.i + 1;
                self.i += 1;
                loop {
                    match self.b.get(self.i) {
                        None => return Err(format!("unterminated string at byte {st}")),
                        Some(b'\\') => self.i += 2,
                        Some(b'"') => break,
                        Some(_) => self.i += 1,
                    }
                }
                self.i += 1;
                Ok(Node::Str(&self.s[st..self.i - 1]))
            }
            _ => {
                let st = self.i;
                while self.i < self.b.len()
                    && !matches!(self.b[self.i], b' ' | b'\n' | b'\t' | b'\r' | b'(' | b')')
                {
                    self.i += 1;
                }
                Ok(Node::Atom(&self.s[st..self.i]))
            }
        }
    }
}

/// `:key value` pairs following the head of a record-style list.
pub fn fields<'a, 'b>(v: &'b [Node<'a>]) -> std::collections::HashMap<&'a str, &'b Node<'a>> {
    let mut m = std::collections::HashMap::new();
    let mut k = 0;
    while k + 1 < v.len() {
        if let Some(a) = v[k].atom()
            && a.starts_with(':')
            && a.len() > 1
        {
            m.insert(&a[1..], &v[k + 1]);
            k += 2;
            continue;
        }
        k += 1;
    }
    m
}

/// `(Fun :path P)` gives `P`.
pub fn fun_path<'a>(n: &Node<'a>) -> Option<&'a str> {
    let v = n.list()?;
    if v.len() == 3 && v[0].atom() == Some("Fun") && v[1].atom() == Some(":path") {
        v[2].atom()
    } else {
        None
    }
}

pub fn render(n: &Node) -> String {
    match n {
        Node::Atom(a) => a.to_string(),
        Node::Str(s) => format!("\"{s}\""),
        Node::List(v) => format!("({})", v.iter().map(render).collect::<Vec<_>>().join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lists_atoms_strings() {
        let mut r = Reader::new("(@ \"a b:1:2: 3:4 (#0)\" (Fun :path m::f))\n\n(x)");
        let f = r.next_form().unwrap().unwrap();
        let v = f.list().unwrap();
        assert_eq!(v[0].atom(), Some("@"));
        assert!(matches!(v[1], Node::Str("a b:1:2: 3:4 (#0)")));
        assert_eq!(fun_path(&v[2]), Some("m::f"));
        assert!(r.next_form().unwrap().is_some());
        assert!(r.next_form().unwrap().is_none());
    }

    #[test]
    fn rejects_unterminated() {
        assert!(Reader::new("(a (b)").next_form().is_err());
        assert!(Reader::new("(a \"b)").next_form().is_err());
    }

    #[test]
    fn fields_pairs() {
        let mut r = Reader::new("(R :a 1 :b (x y) z)");
        let f = r.next_form().unwrap().unwrap();
        let m = fields(&f.list().unwrap()[1..]);
        assert_eq!(m["a"].atom(), Some("1"));
        assert_eq!(render(m["b"]), "(x y)");
    }
}
