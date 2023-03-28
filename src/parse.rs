use std::{error::Error, fmt, path::Path};

use crate::{
    ast::*,
    function::FunctionId,
    lex::{Simple::*, *},
    primitive::Primitive,
    Ident,
};

#[derive(Debug)]
pub enum ParseError {
    Lex(LexError),
    Expected(Vec<Expectation>, Option<Box<Sp<Token>>>),
}

#[derive(Debug)]
pub enum Expectation {
    Term,
    Simple(Simple),
}

impl From<Simple> for Expectation {
    fn from(simple: Simple) -> Self {
        Expectation::Simple(simple)
    }
}

impl fmt::Display for Expectation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expectation::Term => write!(f, "term"),
            Expectation::Simple(s) => write!(f, "`{s}`"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "{e}"),
            ParseError::Expected(exps, found) => {
                write!(f, "expected ")?;
                for (i, exp) in exps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{exp}")?;
                }
                if let Some(found) = found {
                    write!(f, ", found `{}`", found.value)?;
                }
                Ok(())
            }
        }
    }
}

impl Error for ParseError {}

pub type ParseResult<T = ()> = Result<T, Sp<ParseError>>;

pub fn parse(input: &str, path: Option<&Path>) -> (Vec<Item>, Vec<Sp<ParseError>>) {
    let tokens = match lex(input, path) {
        Ok(tokens) => tokens,
        Err(e) => return (Vec::new(), vec![e.map(ParseError::Lex)]),
    };
    let mut items = Vec::new();
    let mut parser = Parser {
        tokens,
        index: 0,
        errors: Vec::new(),
    };
    loop {
        match parser.try_item() {
            Some(item) => items.push(item),
            None => {
                if parser.try_exact(Newline).is_none() {
                    break;
                }
                let mut newline_item = false;
                while parser.try_exact(Newline).is_some() {
                    newline_item = true;
                }
                if newline_item {
                    items.push(Item::Newlines);
                }
            }
        }
    }
    (items, parser.errors)
}

struct Parser {
    tokens: Vec<Sp<crate::lex::Token>>,
    index: usize,
    errors: Vec<Sp<ParseError>>,
}

impl Parser {
    fn next_token_map<'a, T: 'a>(
        &'a mut self,
        f: impl FnOnce(&'a Token) -> Option<T>,
    ) -> Option<Sp<T>> {
        let token = self.tokens.get(self.index)?;
        if let Some(value) = f(&token.value) {
            self.index += 1;
            Some(token.span.clone().sp(value))
        } else {
            None
        }
    }
    fn try_exact(&mut self, token: impl Into<Token>) -> Option<Span> {
        let token = token.into();
        self.next_token_map(|t| (t == &token).then_some(()))
            .map(|t| t.span)
    }
    fn last_span(&self) -> Span {
        if let Some(token) = self.tokens.get(self.index) {
            token.span.clone()
        } else {
            let mut span = self.tokens.last().unwrap().span.clone();
            if let Span::Code(span) = &mut span {
                span.start = span.end;
                if self.tokens.len() > span.end.pos {
                    span.end.pos += 1;
                    span.end.col += 1;
                }
            }
            span
        }
    }
    fn expected<I: Into<Expectation>>(
        &self,
        expectations: impl IntoIterator<Item = I>,
    ) -> Sp<ParseError> {
        self.last_span().sp(ParseError::Expected(
            expectations.into_iter().map(Into::into).collect(),
            self.tokens.get(self.index).cloned().map(Box::new),
        ))
    }
    #[allow(unused)]
    fn expected_continue<I: Into<Expectation>>(
        &mut self,
        expectations: impl IntoIterator<Item = I>,
    ) {
        let err = self.last_span().sp(ParseError::Expected(
            expectations.into_iter().map(Into::into).collect(),
            None,
        ));
        self.errors.push(err);
    }
    fn try_item(&mut self) -> Option<Item> {
        Some(if let Some(binding) = self.try_binding() {
            Item::Binding(binding)
        } else if let Some(words) = self.try_words() {
            Item::Words(words)
        } else if let Some(comment) = self.next_token_map(Token::as_comment) {
            Item::Comment(comment.value.into())
        } else {
            return None;
        })
    }
    fn try_binding(&mut self) -> Option<Binding> {
        Some(if let Some(ident) = self.try_ident() {
            if self.try_exact(Equal).is_none() && self.try_exact(LeftArrow).is_none() {
                self.index -= 1;
                return None;
            }
            let words = self.try_words().unwrap_or_default();
            Binding { name: ident, words }
        } else {
            return None;
        })
    }
    fn try_ident(&mut self) -> Option<Sp<Ident>> {
        self.next_token_map(|token| token.as_ident().cloned())
    }
    fn try_words(&mut self) -> Option<Vec<Sp<Word>>> {
        let mut words = Vec::new();
        while let Some(word) = self.try_word() {
            words.push(word);
        }
        if words.is_empty() {
            None
        } else {
            Some(words)
        }
    }
    fn try_word(&mut self) -> Option<Sp<Word>> {
        self.try_strand()
    }
    fn try_strand(&mut self) -> Option<Sp<Word>> {
        let Some(word) = self.try_modified() else {
            return None;
        };
        let mut items = Vec::new();
        while let Some(uspan) = self.try_exact(Underscore) {
            let item = match self.try_modified() {
                Some(item) => item,
                None => {
                    self.errors.push(self.expected([Expectation::Term]));
                    uspan.sp(Word::Primitive(Primitive::Nop))
                }
            };
            items.push(item);
        }
        if items.is_empty() {
            return Some(word);
        }
        items.insert(0, word);
        for item in &mut items {
            if let Word::Func(func) = &item.value {
                if func.body.is_empty() {
                    item.value = Word::Primitive(Primitive::Nop);
                }
            }
        }
        let span = items[0]
            .span
            .clone()
            .merge(items.last().unwrap().span.clone());
        Some(span.sp(Word::Strand(items)))
    }
    fn try_modified(&mut self) -> Option<Sp<Word>> {
        let mut mod_margs = Primitive::ALL
            .into_iter()
            .filter_map(|prim| prim.modifier_args().map(|margs| (prim, margs)))
            .find_map(|(prim, margs)| {
                self.try_exact(prim)
                    .or_else(|| prim.ascii().and_then(|simple| self.try_exact(simple)))
                    .map(|span| (span.sp(prim), margs))
            });
        if mod_margs.is_none() {
            mod_margs = self
                .next_token_map(|token| {
                    token
                        .as_ident()
                        .and_then(|ident| Primitive::from_name(ident.as_str()))
                        .and_then(|prim| prim.modifier_args().map(|margs| (prim, margs)))
                })
                .map(|sp| (sp.span.sp(sp.value.0), sp.value.1));
        }
        let Some((modifier, margs)) = mod_margs else {
            return self.try_term();
        };
        let mut args = Vec::new();
        for _ in 0..margs {
            if let Some(arg) = self.try_modified() {
                args.push(arg);
            } else {
                break;
            }
        }
        Some(if args.is_empty() {
            modifier.map(Word::Primitive)
        } else {
            let span = modifier
                .span
                .clone()
                .merge(args.last().unwrap().span.clone());
            span.sp(Word::Modified(Box::new(Modified {
                modifier,
                words: args,
            })))
        })
    }
    fn try_term(&mut self) -> Option<Sp<Word>> {
        Some(if let Some(prim) = self.try_op() {
            prim.map(Word::Primitive)
        } else if let Some(ident) = self.try_ident() {
            ident.map(Word::Ident)
        } else if let Some(r) = self.next_token_map(Token::as_number) {
            r.map(Into::into).map(Word::Number)
        } else if let Some(c) = self.next_token_map(Token::as_char) {
            c.map(Into::into).map(Word::Char)
        } else if let Some(s) = self.next_token_map(Token::as_string) {
            s.map(Into::into).map(Word::String)
        } else if let Some(expr) = self.try_func() {
            expr
        } else if let Some(expr) = self.try_ref_func() {
            expr
        } else if let Some(start) = self.try_exact(OpenBracket) {
            let items = self.try_words().unwrap_or_default();
            let end = self.expect_close(CloseBracket);
            let span = start.merge(end);
            span.sp(Word::Array(items))
        } else {
            return None;
        })
    }
    fn try_op(&mut self) -> Option<Sp<Primitive>> {
        for prim in Primitive::ALL {
            let op_span = self
                .try_exact(prim)
                .or_else(|| prim.ascii().and_then(|simple| self.try_exact(simple)));
            if let Some(span) = op_span {
                return Some(span.sp(prim));
            }
        }
        None
    }
    fn try_func(&mut self) -> Option<Sp<Word>> {
        let Some(start) = self.try_exact(OpenParen) else {
            return None;
        };
        let body = self.try_words().unwrap_or_default();
        let end = self.expect_close(CloseParen);
        let span = start.merge(end);
        Some(span.clone().sp(Word::Func(Func {
            id: FunctionId::Anonymous(span),
            body,
        })))
    }
    fn try_ref_func(&mut self) -> Option<Sp<Word>> {
        let Some(start) = self.try_exact(OpenCurly) else {
            return None;
        };
        let body = self.try_words().unwrap_or_default();
        let end = self.expect_close(CloseCurly);
        let span = start.merge(end);
        Some(span.clone().sp(Word::RefFunc(Func {
            id: FunctionId::Anonymous(span),
            body,
        })))
    }
    fn expect_close(&mut self, simple: Simple) -> Span {
        if let Some(span) = self.try_exact(simple) {
            span
        } else {
            self.errors
                .push(self.expected([Expectation::Term, Expectation::Simple(simple)]));
            self.last_span()
        }
    }
}
