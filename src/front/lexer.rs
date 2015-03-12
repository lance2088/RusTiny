//! The lexer: split the source into a stream of tokens

use std::borrow::ToOwned;
use ast::{BinOp, UnOp, Spanned};
use driver::{session, fatal_at};
use driver::codemap::Loc;
use front::tokens::{Token, lookup_keyword};


pub struct Lexer<'a> {
    source: &'a str,
    len: usize,

    pos: usize,
    curr: Option<char>,

    lineno: usize
}

impl<'a> Lexer<'a> {
    // --- Lexer: The public API ------------------------------------------------

    /// Create a new lexer from a given string and file name
    #[allow(unused_variables)]
    pub fn new(source: &'a str, file: &'a str) -> Lexer<'a> {
        Lexer {
            source: source,
            len: source.len(),

            pos: 0,
            curr: Some(source.char_at(0)),

            lineno: 1
        }
    }

    /// Get the next token
    pub fn next_token(&mut self) -> Spanned<Token> {
        while !self.is_eof() {
            // Read the next token as long as the lexer requests us to do so
            if let Some(token) = self.read_token() {
                return token;
            }
        }

        Spanned::new(Token::EOF, self.pos as u32, self.pos as u32)
    }

    /// Tokenize the string into a vector. Used for testing
    #[allow(dead_code)]
    pub fn tokenize(&mut self) -> Vec<Spanned<Token>> {
        let mut tokens = vec![];

        while !self.is_eof() {
            debug!("Processing {:?}", self.curr);

            if let Some(t) = self.read_token() {
                tokens.push(t);
            }

            debug!("So far: {:?}", tokens)
        }

        tokens
    }

    // --- Lexer: Helpers -------------------------------------------------------

    /// Report a fatal error back to the user
    fn fatal(&self, msg: String) -> ! {
        fatal_at(msg, self.get_source())
    }

    /// Are we done yet?
    fn is_eof(&self) -> bool {
        self.curr.is_none()
    }

    /// Get the current source position we're at
    pub fn get_source(&self) -> Loc {
        Loc {
            line: self.lineno as u32,
            col: 0  // FIXME: Column number resolution
        }
    }

    // --- Lexer: Character processing ------------------------------------------

    /// Move along to the next character
    fn bump(&mut self) {
        self.curr = self.nextch();
        self.pos = self.nextch_index();

        debug!("Moved on to {:?}", self.curr)
    }

    fn nextch_index(&self) -> usize {
        // When encountering multi-byte UTF-8, we may stop in the middle
        // of it. Fast forward till we see the next actual char or EOF

        let mut new_pos = self.pos + 1;
        while !self.source.is_char_boundary(new_pos)
                && self.pos < self.len {
            new_pos += 1;
        }

        new_pos
    }

    /// Take a look at the next character without consuming it
    fn nextch(&self) -> Option<char> {
        let new_pos = self.nextch_index();

        if new_pos < self.len {
            Some(self.source.char_at(new_pos))
        } else {
            None
        }
    }

    /// An escaped representation of the current character
    fn curr_escaped(&self) -> String {
        match self.curr {
            Some(c) => c.escape_default().collect(),
            None    => "EOF".to_owned()
        }
    }

    /// Consume an expected character or report an error
    fn expect(&mut self, expect: char) {
        if self.curr != Some(expect) {
            // Build error message
            let expect_str = format!("`{}`", expect);
            let found_str = match self.curr {
                Some(_) => format!("'{}'", self.curr_escaped()),
                None    => String::from_str("EOF")
            };

            self.fatal(format!("Expected `{}`, found `{}`",
                               expect_str, found_str))
        }

        // Consume the current character
        self.bump();
    }

    /// Collect & consume all consecutive characters into a string as long as a condition is true
    fn collect<F>(&mut self, cond: F) -> &'a str
            where F: Fn(&char) -> bool
    {
        let start = self.pos;

        debug!("start colleting");

        while let Some(c) = self.curr {
            if cond(&c) {
                self.bump();
            } else {
                break;
            }
        }

        debug!("colleting finished");

        &self.source[start..self.pos]
    }

    /// Consume all consecutive characters matching a condition
    fn eat_all<F>(&mut self, cond: F)
            where F: Fn(&char) -> bool {
        while let Some(c) = self.curr {
            if cond(&c) { self.bump(); }
            else { break; }
        }
    }

    // --- Lexer: Tokenizers ----------------------------------------------------

    /// Skip over a comment string
    fn skip_comment(&mut self) {
        self.eat_all(|c| *c != '\n');
    }

    /// Tokenize an identifier
    fn tokenize_ident(&mut self) -> Token {
        debug!("tokenizing an ident");

        let ident = self.collect(|c| {
            c.is_alphabetic() || c.is_numeric() || *c == '_'
        });

        // Check whether it's a keyword or an identifier
        if let Some(kw) = lookup_keyword(&ident) {
            Token::Keyword(kw)
        } else {
            Token::Ident(session().interner.intern(ident))
        }
    }

    /// Tokenize an integer
    fn tokenize_integer(&mut self) -> Token {
        debug!("tokenizing a digit");

        let integer_str = self.collect(|c| c.is_numeric());
        let integer     = match integer_str.parse() {
            Ok(i) => i,
            Err(_) => self.fatal(format!("invalid integer: {}", integer_str))
        };

        Token::Int(integer)
    }

    /// Tokenize a character. Correctly handles escaped newlines and escaped single quotes
    fn tokenize_char(&mut self) -> Token {
        debug!("tokenizing a char");

        self.bump();  // '\'' matched, move on

        let c = self.curr.unwrap_or_else(|| {
            self.fatal(format!("expected a char, found EOF"));
        });
        let tok = if c == '\\' {
            // Escaped char, let's take a look on one more char
            self.bump();
            match self.curr {
                Some('n')  => Token::Char('\n'),
                Some('\'') => Token::Char('\''),
                Some(c) => self.fatal(format!("unsupported or invalid escape sequence: \\{}", c)),
                None => self.fatal(format!("expected escaped char, found EOF"))
            }
        } else {
            Token::Char(c)
        };
        self.bump();  // Matched a (possibly escaped) character, move along

        // Match closing quote
        self.expect('\'');

        tok
    }

    /// Read the next token and return it
    ///
    /// If `None` is returned, the current token is to be ignored and the
    /// lexer requests the reader to read the next token instead.
    ///
    /// Precondition: self.curr is not None
    fn read_token(&mut self) -> Option<Spanned<Token>> {
        macro_rules! emit(
            ($_self:ident, $( next: $ch:expr => $tok:expr ),* ; default: $default:expr) => (
                {
                    $_self.bump();
                    match $_self.curr {
                        $( Some($ch) => { $_self.bump(); $tok } , )*
                        _ => $default
                    }
                }
            );

            ($_self:ident, $token:expr) => (
                {
                    $_self.bump();
                    $token
                }
            );
        );

        debug!("tokenizing with current character = `{}` at {}",
                 self.curr_escaped(), self.pos);

        let c = self.curr.unwrap();
        let lo = self.pos;

        let token: Token = match c {
            '+' => emit!(self, Token::BinOp(BinOp::Add)),

            '-' => emit!(self, next: '>' => Token::RArrow;
                               default: Token::BinOp(BinOp::Sub)),

            '*' => emit!(self, next: '*' => Token::BinOp(BinOp::Pow);
                               default: Token::BinOp(BinOp::Mul)),

            '/' => emit!(self, next: '/' => { self.skip_comment(); return None };
                               default: Token::BinOp(BinOp::Div)),

            '%' => emit!(self, Token::BinOp(BinOp::Mod)),

            '&' => emit!(self, next: '&' => Token::BinOp(BinOp::And);
                               default: Token::BinOp(BinOp::BitAnd)),

            '|' => emit!(self, next: '|' => Token::BinOp(BinOp::Or);
                               default: Token::BinOp(BinOp::BitOr)),

            '^' => emit!(self, Token::BinOp(BinOp::BitXor)),

            '<' => emit!(self, next: '<' => Token::BinOp(BinOp::Shl),
                               next: '=' => Token::BinOp(BinOp::Le);
                               default: Token::BinOp(BinOp::Lt)),

            '>' => emit!(self, next: '>' => Token::BinOp(BinOp::Shr),
                               next: '=' => Token::BinOp(BinOp::Ge);
                               default: Token::BinOp(BinOp::Gt)),

            '=' => emit!(self, next: '=' => Token::BinOp(BinOp::EqEq);
                               default: Token::Eq),

            '!' => emit!(self, next: '=' => Token::BinOp(BinOp::Ne);
                               default: Token::UnOp(UnOp::Not)),

            '(' => emit!(self, Token::LParen),

            ')' => emit!(self, Token::RParen),

            '{' => emit!(self, Token::LBrace),

            '}' => emit!(self, Token::RBrace),

            ',' => emit!(self, Token::Comma),

            ':' => emit!(self, Token::Colon),

            ';' => emit!(self, Token::Semicolon),

            '\'' => self.tokenize_char(),

            c if c.is_alphabetic()  => self.tokenize_ident(),

            c if c.is_numeric() => self.tokenize_integer(),

            c if c.is_whitespace() => {
                // Skip whitespaces of any type
                if c == '\n' {
                    self.lineno += 1;
                    let offset = if self.nextch() == Some('\r') { 2 } else { 1 };
                    session().codemap.new_line(self.pos as u32 + offset)
                }

                self.bump();
                return None;
            },
            c => self.fatal(format!("unexpected character: `{}`", c))
        };

        debug!("emitted token: `{:?}`", token);

        Some(Spanned::new(token, lo as u32, self.pos as u32))
    }
}