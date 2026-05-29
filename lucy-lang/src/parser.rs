#![allow(unused)]

use std::collections::HashMap;
use std::iter::Peekable;
use std::vec::IntoIter;

use crate::lexer::{SpannedToken, Token};
use crate::operator::Operator;
use crate::span::{Span, Spanned};

pub type SAst = Spanned<AstNode>;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span:    Span,
}

// ---------------------------------------------------------------------------
// Internal macros
// ---------------------------------------------------------------------------

/// Push an error and return `None` from the current function.
/// Used inside helpers that return `Option<T>`.
///
/// ```ignore
/// err_none!(self, span, "Expected '(' after for, got {:?}", tok);
/// ```
macro_rules! err_none {
    ($self:expr, $span:expr, $($arg:tt)*) => {{
        $self.push_error(format!($($arg)*), $span);
        return None;
    }};
}

/// Push an error and return an `AstNode::Error` SAst from the current function.
/// Used inside functions that return `SAst`.
///
/// ```ignore
/// err_node!(self, span, "Unexpected token {:?}", tok);
/// ```
macro_rules! err_node {
    ($self:expr, $span:expr, $($arg:tt)*) => {{
        $self.push_error(format!($($arg)*), $span);
        return Self::spanned(AstNode::Error, $span);
    }};
}

// ---------------------------------------------------------------------------
// Parsing context
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ParsingContext {
    pub no_fn_body:    bool,
    pub current_class: Option<String>,
}

impl ParsingContext {
    fn new() -> Self {
        Self { no_fn_body: false, current_class: None }
    }
}

// ---------------------------------------------------------------------------
// Symbol table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SymbolTable {
    pub macros: HashMap<String, MacroDefinition>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self { macros: HashMap::new() }
    }

    pub fn define_macro(&mut self, name: String, def: MacroDefinition) {
        self.macros.insert(name, def);
    }

    pub fn lookup_macro(&self, name: &str) -> Option<&MacroDefinition> {
        self.macros.get(name)
    }
}

// ---------------------------------------------------------------------------
// Macro AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum MacroDesignator {
    Expr,
    Stmt,
    Ty,
    Pat,
    Ident,
    Block,
    Lifetime,
    Vis,
    Meta,
    Tt,
    Str,
    Int,
    Float,
    Literal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MacroTokenTree {
    Token(Token<'static>, Span),
    Metavar { name: String, designator: MacroDesignator },
    Repetition {
        trees:      Vec<MacroTokenTree>,
        separator:  Option<String>,
        quantifier: RepQuantifier,
    },
    Group { delimiter: char, trees: Vec<MacroTokenTree> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RepQuantifier { ZeroOrMore, OneOrMore }

#[derive(Debug, Clone, PartialEq)]
pub struct MacroArm {
    pub pattern:  Vec<MacroTokenTree>,
    pub template: Vec<MacroTokenTree>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroDefinition {
    pub name: String,
    pub arms: Vec<MacroArm>,
}

// ---------------------------------------------------------------------------
// Type nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum TypeNode {
    NominalType { name: String, generics: Vec<TypeNode> },
    ArrayType   { elem_type: Box<TypeNode> },
    HashMapType { key_type: Box<TypeNode>, value_type: Box<TypeNode> },
    Qualified   { inner: Box<TypeNode>, mutable: bool, borrowed: bool, moved: bool },
    Inferred,
}

// ---------------------------------------------------------------------------
// Binding / pattern nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum BindingNode {
    IdentifierBinding { name: String, ty: TypeNode },
    OrderedBinding    { bindings: Vec<BindingNode> },
    UnorderedBinding  { bindings: Vec<BindingNode> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPattern {
    Binding(BindingNode),
    Expr(SAst),
    CategoryVariant { path: Vec<String>, binding: Option<BindingNode> },
    Default(Option<String>),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body:    Vec<SAst>,
}

// ---------------------------------------------------------------------------
// Class / category members
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ClassMember {
    Field {
        name:      String,
        ty:        TypeNode,
        is_public: bool,
    },
    Method {
        name:        String,
        type_params: Vec<(TypeNode, Option<TypeNode>)>,
        has_self:    bool,
        params:      Vec<BindingNode>,
        return_type: TypeNode,
        body:        Vec<SAst>,
        is_public:   bool,
    },
    OperatorOverload {
        op:          Operator,
        params:      Vec<BindingNode>,
        return_type: TypeNode,
        body:        Vec<SAst>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CategoryVariant {
    Unit  { name: String },
    Tuple { name: String, fields: Vec<TypeNode>, methods: Vec<ClassMember> },
    Class { name: String, fields: Vec<(String, TypeNode)>, methods: Vec<ClassMember> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedTupleDef {
    pub name:    String,
    pub fields:  Vec<TypeNode>,
    pub methods: Vec<ClassMember>,
}

// ---------------------------------------------------------------------------
// Format-string parts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum FmtPart {
    Literal(String),
    Expr(SAst),
}

// ---------------------------------------------------------------------------
// AST node
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum AstNode {
    Identifier(String),

    IntLiteral(i32),
    FloatLiteral(f64),
    StringLiteral(String),
    FmtStringLiteral(Vec<FmtPart>),
    ArrayLiteral(Vec<SAst>),
    HashMapLiteral(Vec<(SAst, SAst)>),
    SelfExpr,

    VarDeclaration {
        binding:    BindingNode,
        init_value: Option<Box<SAst>>,
    },
    Assignment {
        left:  Box<SAst>,
        right: Box<SAst>,
    },
    FunctionDeclaration {
        name:        String,
        type_params: Vec<(TypeNode, Option<TypeNode>)>,
        params:      Vec<BindingNode>,
        return_type: TypeNode,
        body:        Vec<SAst>,
    },
    ReturnStmt { value: Option<Box<SAst>> },

    ModuleStmt  { name: String },
    UseStmt        { base_path: Box<SAst>, used: Vec<(String, String)> },
    Public(Box<SAst>),
    Borrowed(Box<SAst>),
    Moved(Box<SAst>),
    Typeof(Box<SAst>),
    Throw(Box<SAst>),

    BinaryOperation  { op: Operator, left: Box<SAst>, right: Box<SAst> },
    UnaryOperation   { op: Operator, right: Box<SAst> },
    ComputedIndex    { indexee: Box<SAst>, index: Box<SAst> },
    DotIndex         { indexee: Box<SAst>, index: Box<SAst> },
    MethodCall       { indexee: Box<SAst>, index: Box<SAst> },
    // NamespaceIndex   { indexee: Box<SAst>, index: Box<SAst> },
    FunctionCall     { callee: Box<SAst>, args: Vec<SAst> },
    ClassLiteral     { ty: Box<SAst>, fields: Vec<(String, SAst)> },
    TypeInstantiation { callee: Box<SAst>, type_args: Vec<TypeNode> },
    TypeCast         { left: Box<SAst>, right: TypeNode },

    ConditionalBranch {
        condition: Option<Box<SAst>>,
        body:      Vec<SAst>,
        next:      Option<Box<SAst>>,
    },
    WhileLoop  { condition: Box<SAst>, body: Vec<SAst> },
    MatchStmt  { matchee: Box<SAst>, arms: Vec<MatchArm> },
    ForLoop    { binding: BindingNode, iterator: Box<SAst>, body: Vec<SAst> },

    ClassDefinition      { name: String, members: Vec<ClassMember> },
    NamedTupleDefinition(NamedTupleDef),
    CategoryDefinition   { name: String, variants: Vec<CategoryVariant> },

    MacroDefinitionNode(MacroDefinition),
    MacroInvocation { name: String, args: Vec<MacroTokenTree> },

    /// Sentinel emitted when parsing fails but we want to continue.
    Error,

    Program(Vec<SAst>),
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

type PeekIter = Peekable<IntoIter<SpannedToken<'static>>>;

pub struct LucyParser {
    tokens:  PeekIter,
    pub symbols: SymbolTable,
    pub errors:  Vec<ParseError>,
}

impl LucyParser {
    pub fn new(tokens: Vec<SpannedToken<'static>>) -> Self {
        Self {
            tokens:  tokens.into_iter().peekable(),
            symbols: SymbolTable::new(),
            errors:  Vec::new(),
        }
    }

    /// Returns `true` if parsing produced no errors.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn push_error(&mut self, message: String, span: Span) {
        self.errors.push(ParseError { message, span });
    }
}

// ---------------------------------------------------------------------------
// Low-level token helpers
// ---------------------------------------------------------------------------

impl LucyParser {
    fn consume(&mut self) -> Option<SpannedToken<'static>> {
        self.tokens.next()
    }

    fn peek_tok(&mut self) -> Option<&Token<'static>> {
        self.tokens.peek().map(|(t, _)| t)
    }

    fn peek_spanned(&mut self) -> Option<&SpannedToken<'static>> {
        self.tokens.peek()
    }

    fn peek_some(&mut self) -> Token<'static> {
        self.tokens.peek()
            .map(|(t, _)| t.clone())
            .unwrap_or(Token::END)
    }

    fn peek_span(&mut self) -> Span {
        self.tokens.peek().map(|(_, s)| *s).unwrap_or(Span::dummy())
    }

    fn expect_any_some(&mut self) -> Option<SpannedToken<'static>> {
        match self.tokens.next() {
            Some(t) => Some(t),
            None => {
                self.push_error("Unexpected end of input".to_string(), Span::dummy());
                None
            }
        }
    }

    fn consume_end(&mut self, end_span: &mut Span) {
        if let Some((_, s)) = self.consume() {
            *end_span = s;
        } else {
            self.push_error("No token".into(), *end_span);
        }
    }

    /// Consume the next token, recording an error (and returning `None`) if it
    /// does not match `expected`.
    fn expect_some(
        &mut self,
        expected: Token<'static>,
        msg: &str,
    ) -> Option<SpannedToken<'static>> {
        match self.tokens.next() {
            None => {
                self.push_error(
                    format!("Expected {:?} but got end of input: {}", expected, msg),
                    Span::dummy(),
                );
                None
            }
            Some(got) if got.0 != expected => {
                self.push_error(
                    format!("Expected {:?}, got {:?}: {}", expected, got.0, msg),
                    got.1,
                );
                None
            }
            Some(got) => Some(got),
        }
    }

    fn spanned(node: AstNode, span: Span) -> SAst {
        Spanned::new(node, span)
    }
}

// ---------------------------------------------------------------------------
// Body / binding / type helpers
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_body(&mut self, ctx: &ParsingContext) -> Vec<SAst> {
        let mut body = Vec::new();
        loop {
            match self.peek_some() {
                Token::END => { self.consume(); break; }
                _          => body.push(self.parse_stmt(ctx)),
            }
        }
        body
    }

    /// Returns `None` on parse failure (error already recorded).
    fn parse_binding(&mut self, ctx: &ParsingContext) -> Option<BindingNode> {
        match self.peek_some() {
            Token::IDENT(_) | Token::SELF => {
                let (tok, span) = self.expect_any_some()?;
                let name = match tok {
                    Token::IDENT(s) => s.to_string(),
                    Token::SELF     => "self".to_string(),
                    _ => unreachable!(),
                };
                let ty = if let Token::PUNCT(":") = self.peek_some() {
                    self.consume();
                    self.parse_type(ctx)?
                } else {
                    TypeNode::Inferred
                };
                Some(BindingNode::IdentifierBinding { name, ty })
            }

            Token::PAREN("(") => {
                self.consume();
                let mut bindings = Vec::new();
                loop {
                    match self.peek_some() {
                        Token::PAREN(")") => { self.consume(); break; }
                        Token::PUNCT(",") => { self.consume(); }
                        _ => bindings.push(self.parse_binding(ctx)?),
                    }
                }
                Some(BindingNode::OrderedBinding { bindings })
            }

            Token::PAREN("{") => {
                self.consume();
                let mut bindings = Vec::new();
                loop {
                    match self.peek_some() {
                        Token::PAREN("}") => { self.consume(); break; }
                        Token::PUNCT(",") => { self.consume(); }
                        _ => bindings.push(self.parse_binding(ctx)?),
                    }
                }
                Some(BindingNode::UnorderedBinding { bindings })
            }

            other => {
                let span = self.peek_span();
                err_none!(self, span, "Unknown binding initializer: {:?}", other);
            }
        }
    }

    /// Returns `None` on parse failure (error already recorded).
    fn parse_type(&mut self, ctx: &ParsingContext) -> Option<TypeNode> {
        let mut is_mutable  = false;
        let mut is_borrowed = false;
        let mut is_moved    = false;

        loop {
            match self.peek_some() {
                Token::MUTABLE => { self.consume(); is_mutable  = true; }
                Token::AND     => { self.consume(); is_borrowed = true; }
                Token::ARROW   => { self.consume(); is_moved    = true; }
                _              => break,
            }
        }

        let (tok, span) = self.expect_any_some()?;

        let base = match tok {
            Token::IDENT(name) => {
                let name = name.to_string();
                let mut generics = Vec::new();
                if let Token::BINOP("<") = self.peek_some() {
                    self.consume();
                    loop {
                        match self.peek_some() {
                            Token::BINOP(">") => { self.consume(); break; }
                            Token::PUNCT(",") => { self.consume(); }
                            _                 => generics.push(self.parse_type(ctx)?),
                        }
                    }
                }
                TypeNode::NominalType { name, generics }
            }

            Token::SELFTYPE => {
                match &ctx.current_class {
                    Some(name) => TypeNode::NominalType { name: name.clone(), generics: vec![] },
                    None => {
                        self.push_error("'Self' used outside of a class body".to_string(), span);
                        return None;
                    }
                }
            }

            Token::PAREN("[") => {
                let elem_type = self.parse_type(ctx)?;
                self.expect_some(Token::PAREN("]"), "Expected ']' after array element type")?;
                TypeNode::ArrayType { elem_type: Box::new(elem_type) }
            }

            Token::PAREN("{") => {
                self.expect_some(Token::PAREN("["), "Expected '[' after '{' in hashmap type")?;
                let key_type = self.parse_type(ctx)?;
                self.expect_some(Token::PAREN("]"), "Expected ']' after hashmap key type")?;
                self.expect_some(Token::PUNCT(":"),  "Expected ':' after hashmap key type")?;
                let value_type = self.parse_type(ctx)?;
                self.expect_some(Token::PAREN("}"), "Expected '}' to close hashmap type")?;
                TypeNode::HashMapType {
                    key_type:   Box::new(key_type),
                    value_type: Box::new(value_type),
                }
            }

            other => {
                self.push_error(format!("Unhandled type token: {:?}", other), span);
                return None;
            }
        };

        Some(if is_mutable || is_borrowed || is_moved {
            TypeNode::Qualified {
                inner:    Box::new(base),
                mutable:  is_mutable,
                borrowed: is_borrowed,
                moved:    is_moved,
            }
        } else {
            base
        })
    }

    fn parse_call_args(&mut self, ctx: &ParsingContext) -> Vec<SAst> {
        let mut args = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _                 => args.push(self.parse_expr(ctx)),
            }
        }
        args
    }
}

// ---------------------------------------------------------------------------
// Macro definitions & invocations
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_macro_definition(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let name = match self.expect_any_some() {
            Some((Token::IDENT(s), _)) => s.to_string(),
            Some((other, span))        => err_node!(self, span, "Expected macro name, got {:?}", other),
            None                       => return Self::spanned(AstNode::Error, start),
        };

        if let Token::BANG = self.peek_some() {
            self.consume();
        }

        let mut arms     = Vec::new();
        let mut end_span = start;

        loop {
            match self.peek_some() {
                Token::END => {self.consume_end(&mut end_span); break;}
                Token::PUNCT(",") => { self.consume(); }
                _ => {
                    let pattern = self.parse_macro_token_trees_until_fat_arrow();
                    if self.expect_some(Token::FATARROW, "Expected '=>' in macro arm").is_none() {
                        // skip to next arm or end
                        continue;
                    }
                    let template = self.parse_macro_token_trees_until_arm_end();
                    arms.push(MacroArm { pattern, template });
                }
            }
        }

        let def = MacroDefinition { name: name.clone(), arms };
        self.symbols.define_macro(name, def.clone());
        Self::spanned(AstNode::MacroDefinitionNode(def), start.merge(end_span))
    }

    fn parse_macro_token_trees_until_fat_arrow(&mut self) -> Vec<MacroTokenTree> {
        self.parse_macro_token_trees_inner(|tok| matches!(tok, Token::FATARROW))
    }

    fn parse_macro_token_trees_until_arm_end(&mut self) -> Vec<MacroTokenTree> {
        self.parse_macro_token_trees_inner(|tok| matches!(tok, Token::END))
    }

    fn parse_macro_token_trees_inner(
        &mut self,
        stop_at: impl Fn(&Token) -> bool,
    ) -> Vec<MacroTokenTree> {
        let mut trees = Vec::new();
        loop {
            match self.peek_tok() {
                None => break,
                Some(t) if stop_at(t) => break,
                _ => {}
            }
            if let Some(tree) = self.parse_one_macro_token_tree() {
                trees.push(tree);
            } else {
                break;
            }
        }
        trees
    }

    /// Returns `None` on failure (error already recorded).
    fn parse_one_macro_token_tree(&mut self) -> Option<MacroTokenTree> {
        match self.peek_some() {
            Token::MACRO_REP_START => {
                self.consume();
                let inner = self.parse_macro_token_trees_inner(
                    |t| matches!(t, Token::PAREN(")")),
                );
                self.consume(); // consume `$)`

                let separator = match self.peek_some() {
                    Token::PUNCT(",") | Token::PUNCT(";") => {
                        if let Some((tok, _)) = self.consume()
                        {
                            Some(match tok {
                                Token::PUNCT(s) => s.to_string(),
                                _ => unreachable!(),
                            })
                        }
                        else {None}
                    }
                    _ => None,
                };

                let quantifier = match self.peek_some() {
                    Token::UNARY("*") | Token::BINOP("*") => {
                        self.consume();
                        RepQuantifier::ZeroOrMore
                    }
                    Token::UNARY("+") | Token::BINOP("+") => {
                        self.consume();
                        RepQuantifier::OneOrMore
                    }
                    other => {
                        let span = self.peek_span();
                        self.push_error(
                            format!("Expected * or + after macro repetition, got {:?}", other),
                            span,
                        );
                        return None;
                    }
                };

                Some(MacroTokenTree::Repetition { trees: inner, separator, quantifier })
            }

            Token::MACRO_VAR(name) => {
                let name = name.to_string();
                self.consume();
                if let Token::PUNCT(":") = self.peek_some() {
                    self.consume();
                    let designator = self.parse_designator()?;
                    Some(MacroTokenTree::Metavar { name, designator })
                } else {
                    Some(MacroTokenTree::Metavar { name, designator: MacroDesignator::Tt })
                }
            }

            Token::PAREN("(") => {
                self.consume();
                let inner = self.parse_macro_token_trees_inner(
                    |t| matches!(t, Token::PAREN(")")),
                );
                self.consume();
                Some(MacroTokenTree::Group { delimiter: '(', trees: inner })
            }

            Token::PAREN("[") => {
                self.consume();
                let inner = self.parse_macro_token_trees_inner(
                    |t| matches!(t, Token::PAREN("]")),
                );
                self.consume();
                Some(MacroTokenTree::Group { delimiter: '[', trees: inner })
            }

            Token::PAREN("{") => {
                self.consume();
                let inner = self.parse_macro_token_trees_inner(
                    |t| matches!(t, Token::PAREN("}")),
                );
                self.consume();
                Some(MacroTokenTree::Group { delimiter: '{', trees: inner })
            }

            _ => {
                if let Some((tok, span)) = self.consume()
                {
                    Some(MacroTokenTree::Token(tok, span))
                }
                else {None}
            }
        }
    }

    /// Returns `None` on an unknown designator (error already recorded).
    fn parse_designator(&mut self) -> Option<MacroDesignator> {
        let (tok, span) = self.expect_any_some()?;
        let d = match tok {
            Token::IDENT("expr")     => MacroDesignator::Expr,
            Token::IDENT("stmt")     => MacroDesignator::Stmt,
            Token::IDENT("ty")       => MacroDesignator::Ty,
            Token::IDENT("pat")      => MacroDesignator::Pat,
            Token::IDENT("ident")    => MacroDesignator::Ident,
            Token::IDENT("block")    => MacroDesignator::Block,
            Token::IDENT("lifetime") => MacroDesignator::Lifetime,
            Token::IDENT("vis")      => MacroDesignator::Vis,
            Token::IDENT("meta")     => MacroDesignator::Meta,
            Token::IDENT("tt")       => MacroDesignator::Tt,
            Token::IDENT("str")      => MacroDesignator::Str,
            Token::IDENT("int")      => MacroDesignator::Int,
            Token::IDENT("float")    => MacroDesignator::Float,
            Token::IDENT("literal")  => MacroDesignator::Literal,
            other => {
                self.push_error(format!("Unknown macro designator: {:?}", other), span);
                return None;
            }
        };
        Some(d)
    }

    fn parse_macro_invocation(&mut self, name: String, span: Span) -> SAst {
        match self.peek_some() {
            Token::BANG => { self.consume(); }
            other => err_node!(
                self, span,
                "Expected '!' after macro name '{}', got {:?}", name, other
            ),
        }

        let args = match self.peek_some() {
            Token::PAREN("(") | Token::PAREN("[") | Token::PAREN("{") => {
                match self.parse_one_macro_token_tree() {
                    Some(tree) => vec![tree],
                    None       => vec![],
                }
            }
            _ => match self.parse_one_macro_token_tree() {
                Some(tree) => vec![tree],
                None       => vec![],
            },
        };

        Self::spanned(AstNode::MacroInvocation { name, args }, span)
    }
}

// ---------------------------------------------------------------------------
// Top-level / statement dispatch
// ---------------------------------------------------------------------------

impl LucyParser {
    pub fn parse_file_source(&mut self) -> SAst {
        let start = self.peek_span();
        let mut stmts = Vec::new();
        let ctx = ParsingContext::new();
        while self.peek_tok().is_some() {
            stmts.push(self.parse_stmt(&ctx));
        }
        let end_span = stmts.last().map(|s| s.span).unwrap_or(start);
        Self::spanned(AstNode::Program(stmts), start.merge(end_span))
    }

    fn parse_stmt(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        match self.peek_some() {
            Token::FN       => { self.consume(); self.parse_fun_declaration(ctx) }
            Token::DECLARE  => { self.consume(); self.parse_var_declaration(ctx) }
            Token::FOR      => { self.consume(); self.parse_for_loop(ctx) }
            Token::RETURN   => { self.consume(); self.parse_ret(ctx) }
            Token::MODULE => { self.consume(); self.parse_module(ctx) }
            Token::USE      => { self.consume(); self.parse_use(ctx) }
            Token::PUB      => { self.consume(); self.parse_public(ctx) }
            Token::CLASS    => { self.consume(); self.parse_class_definition(ctx) }
            Token::WHILE    => { self.consume(); self.parse_while_loop(ctx) }
            Token::IF       => { self.consume(); self.parse_conditional_branch(ctx) }
            Token::MATCH    => { self.consume(); self.parse_match_stmt(ctx) }
            Token::MACRO    => { self.consume(); self.parse_macro_definition(ctx) }
            Token::TUPLE    => { self.consume(); self.parse_named_tuple_definition(ctx) }
            Token::CATEGORY => { self.consume(); self.parse_category_definition(ctx) }
            Token::THROW    => {
                self.consume();
                let val  = self.parse_expr(ctx);
                let span = start.merge(val.span);
                Self::spanned(AstNode::Throw(Box::new(val)), span)
            }
            _ => self.parse_expr(ctx),
        }
    }
}

// ---------------------------------------------------------------------------
// Named-tuple definitions
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_named_tuple_definition(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let name = match self.expect_any_some() {
            Some((Token::IDENT(s), _)) => s.to_string(),
            Some((other, span))        => err_node!(self, span, "Expected tuple name, got {:?}", other),
            None                       => return Self::spanned(AstNode::Error, start),
        };

        if self.expect_some(Token::PAREN("("), "Expected '(' after tuple name").is_none() {
            return Self::spanned(AstNode::Error, start);
        }

        let mut fields = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _ => match self.parse_type(ctx) {
                    Some(ty) => fields.push(ty),
                    None     => return Self::spanned(AstNode::Error, start),
                },
            }
        }

        let mut class_ctx = ctx.clone();
        class_ctx.current_class = Some(name.clone());

        let methods  = self.parse_optional_method_block(&class_ctx, &name);
        let end_span = self.peek_span();

        Self::spanned(
            AstNode::NamedTupleDefinition(NamedTupleDef { name, fields, methods }),
            start.merge(end_span),
        )
    }

    fn parse_optional_method_block(
        &mut self,
        ctx:        &ParsingContext,
        class_name: &str,
    ) -> Vec<ClassMember> {
        let mut methods = Vec::new();
        loop {
            match self.peek_some() {
                Token::FN => {
                    self.consume();
                    if let Some(m) = self.parse_class_method(ctx, class_name, false) {
                        methods.push(m);
                    }
                }
                Token::PUB => {
                    self.consume();
                    match self.peek_some() {
                        Token::FN => {
                            self.consume();
                            if let Some(m) = self.parse_class_method(ctx, class_name, true) {
                                methods.push(m);
                            }
                        }
                        other => {
                            let span = self.peek_span();
                            self.push_error(
                                format!("Expected 'fn' after 'pub' in method block, got {:?}", other),
                                span,
                            );
                            break;
                        }
                    }
                }
                Token::END => { self.consume(); break; }
                _          => break,
            }
        }
        methods
    }
}

// ---------------------------------------------------------------------------
// Category definitions
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_category_definition(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let name = match self.expect_any_some() {
            Some((Token::IDENT(s), _)) => s.to_string(),
            Some((other, span))        => err_node!(self, span, "Expected category name, got {:?}", other),
            None                       => return Self::spanned(AstNode::Error, start),
        };

        let mut cat_ctx = ctx.clone();
        cat_ctx.current_class = Some(name.clone());

        let mut variants = Vec::new();
        let mut end_span = start;

        loop {
            match self.peek_some() {
                Token::END => {self.consume_end(&mut end_span); break;}
                Token::TUPLE => {
                    self.consume();
                    match self.parse_category_tuple_variant(&cat_ctx) {
                        Some(v) => variants.push(v),
                        None    => break,
                    }
                }
                Token::CLASS => {
                    self.consume();
                    match self.parse_category_class_variant(&cat_ctx) {
                        Some(v) => variants.push(v),
                        None    => break,
                    }
                }
                Token::IDENT(_) => {
                    if let Some((tok, _)) = self.consume()
                    {
                        let vname = match tok {
                            Token::IDENT(s) => s.to_string(),
                            _ => unreachable!(),
                        };
                        variants.push(CategoryVariant::Unit { name: vname });
                    }
                    else
                    {
                        self.push_error("No token".into(), end_span);
                    }
                }
                other => {
                    let span = self.peek_span();
                    err_node!(self, span, "Unexpected token in category body: {:?}", other);
                }
            }
        }

        Self::spanned(AstNode::CategoryDefinition { name, variants }, start.merge(end_span))
    }

    fn parse_category_tuple_variant(&mut self, ctx: &ParsingContext) -> Option<CategoryVariant> {
        let (tok, span) = self.expect_any_some()?;
        let name = match tok {
            Token::IDENT(s) => s.to_string(),
            other           => err_none!(self, span, "Expected variant name, got {:?}", other),
        };

        self.expect_some(Token::PAREN("("), "Expected '(' after tuple variant name")?;

        let mut fields = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _                 => fields.push(self.parse_type(ctx)?),
            }
        }

        let methods = if let Token::PUNCT(":") = self.peek_some() {
            self.consume();
            self.parse_optional_method_block(ctx, &name)
        } else {
            Vec::new()
        };

        Some(CategoryVariant::Tuple { name, fields, methods })
    }

    fn parse_category_class_variant(&mut self, ctx: &ParsingContext) -> Option<CategoryVariant> {
        let (tok, span) = self.expect_any_some()?;
        let name = match tok {
            Token::IDENT(s) => s.to_string(),
            other           => err_none!(self, span, "Expected variant name, got {:?}", other),
        };

        self.expect_some(Token::PAREN("{"), "Expected '{' after struct variant name")?;

        let mut fields = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN("}") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                Token::IDENT(_) => {
                    let (ftok, fspan) = self.expect_any_some()?;
                    let fname = match ftok {
                        Token::IDENT(s) => s.to_string(),
                        _               => unreachable!(),
                    };
                    self.expect_some(Token::PUNCT(":"), "Expected ':' after struct field name")?;
                    let ty = self.parse_type(ctx)?;
                    fields.push((fname, ty));
                }
                other => {
                    let span = self.peek_span();
                    err_none!(self, span, "Unexpected token in struct variant, got {:?}", other);
                }
            }
        }

        let methods = if let Token::PUNCT(":") = self.peek_some() {
            self.consume();
            self.parse_optional_method_block(ctx, &name)
        } else {
            Vec::new()
        };

        Some(CategoryVariant::Class { name, fields, methods })
    }
}

// ---------------------------------------------------------------------------
// Class definitions
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_class_definition(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let name = match self.expect_any_some() {
            Some((Token::IDENT(s), _)) => s.to_string(),
            Some((other, span))        => err_node!(self, span, "Expected class name, got {:?}", other),
            None                       => return Self::spanned(AstNode::Error, start),
        };

        let mut class_ctx = ctx.clone();
        class_ctx.current_class = Some(name.clone());

        let mut members  = Vec::new();
        let mut end_span = start;

        loop {
            match self.peek_some() {
                Token::END => {self.consume_end(&mut end_span); break;}
                Token::PUB => {
                    self.consume();
                    match self.peek_some() {
                        Token::FN => {
                            self.consume();
                            if let Some(m) = self.parse_class_method(&class_ctx, &name, true) {
                                members.push(m);
                            }
                        }
                        Token::IDENT(_) => {
                            if let Some(m) = self.parse_class_field(&class_ctx, true) {
                                members.push(m);
                            }
                        }
                        other => {
                            let span = self.peek_span();
                            err_node!(self, span, "Expected fn or field after pub in class, got {:?}", other);
                        }
                    }
                }
                Token::OPERATOR => {
                    self.consume();
                    if let Some(m) = self.parse_operator_overload(&class_ctx) {
                        members.push(m);
                    }
                }
                Token::FN => {
                    self.consume();
                    if let Some(m) = self.parse_class_method(&class_ctx, &name, false) {
                        members.push(m);
                    }
                }
                Token::IDENT(_) => {
                    if let Some(m) = self.parse_class_field(&class_ctx, false) {
                        members.push(m);
                    }
                }
                other => {
                    let span = self.peek_span();
                    err_node!(self, span, "Unexpected token in class body: {:?}", other);
                }
            }
        }

        Self::spanned(AstNode::ClassDefinition { name, members }, start.merge(end_span))
    }

    fn parse_operator_overload(&mut self, ctx: &ParsingContext) -> Option<ClassMember> {
        let (tok, span) = self.expect_any_some()?;
        let op_str = match tok {
            Token::BINOP(s) | Token::UNARY(s) => s,
            other => err_none!(self, span, "Unknown operator {:?}", other),
        };

        self.expect_some(Token::PAREN("("), "Expected '(' after operator")?;

        let mut params = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _                 => params.push(self.parse_binding(ctx)?),
            }
        }

        let return_type = if let Token::PUNCT(":") = self.peek_some() {
            self.consume();
            self.parse_type(ctx)?
        } else {
            TypeNode::Inferred
        };

        let body = self.parse_body(ctx);

        let operator = match op_str {
            "+" => Operator::Add,
            "-" => Operator::Sub,
            "/" => Operator::Div,
            "*" => Operator::Mul,
            other => {
                self.push_error(format!("Unknown operator '{}'", other), span);
                return None;
            }
        };

        Some(ClassMember::OperatorOverload { op: operator, params, return_type, body })
    }

    fn parse_class_field(&mut self, ctx: &ParsingContext, is_public: bool) -> Option<ClassMember> {
        let (tok, span) = self.expect_any_some()?;
        let name = match tok {
            Token::IDENT(s) => s.to_string(),
            other           => err_none!(self, span, "Expected field name, got {:?}", other),
        };
        self.expect_some(Token::PUNCT(":"), "Expected ':' after field name")?;
        let ty = self.parse_type(ctx)?;
        Some(ClassMember::Field { name, ty, is_public })
    }

    fn parse_class_method(
        &mut self,
        ctx:         &ParsingContext,
        _class_name: &str,
        is_public:   bool,
    ) -> Option<ClassMember> {
        let (tok, span) = self.expect_any_some()?;
        let method_name = match tok {
            Token::IDENT(s) => s.to_string(),
            other           => err_none!(self, span, "Expected method name, got {:?}", other),
        };

        let mut type_params = Vec::new();
        if let Token::BINOP("<") = self.peek_some() {
            self.consume();
            loop {
                match self.peek_some() {
                    Token::BINOP(">") => { self.consume(); break; }
                    Token::PUNCT(",") => { self.consume(); }
                    _ => {
                        let node       = self.parse_type(ctx)?;
                        let constraint = if let Token::PUNCT(":") = self.peek_some() {
                            self.consume();
                            Some(self.parse_type(ctx)?)
                        } else {
                            None
                        };
                        type_params.push((node, constraint));
                    }
                }
            }
        }

        self.expect_some(Token::PAREN("("), "Expected '(' after method name")?;

        let has_self = matches!(self.peek_some(), Token::SELF);
        if has_self {
            self.consume();
            if let Token::PUNCT(",") = self.peek_some() { self.consume(); }
        }

        let mut params = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _                 => params.push(self.parse_binding(ctx)?),
            }
        }

        let return_type = if let Token::PUNCT(":") = self.peek_some() {
            self.consume();
            self.parse_type(ctx)?
        } else {
            TypeNode::Inferred
        };

        let body = self.parse_body(ctx);

        Some(ClassMember::Method {
            name: method_name,
            type_params,
            has_self,
            params,
            return_type,
            body,
            is_public,
        })
    }
}

// ---------------------------------------------------------------------------
// Visibility / namespace / use
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_public(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        let inner = match self.peek_some() {
            Token::CLASS    => { self.consume(); self.parse_class_definition(ctx) }
            Token::CATEGORY => { self.consume(); self.parse_category_definition(ctx) }
            Token::TUPLE    => { self.consume(); self.parse_named_tuple_definition(ctx) }
            Token::MACRO    => { self.consume(); self.parse_macro_definition(ctx) }
            Token::IDENT(..) => self.parse_var_declaration(ctx),
            Token::FN       => { self.consume(); self.parse_fun_declaration(ctx) }
            other => err_node!(self, start, "Cannot export this statement as public: {:?}", other),
        };
        let span = start.merge(inner.span);
        Self::spanned(AstNode::Public(Box::new(inner)), span)
    }

    fn parse_module(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        match self.expect_any_some() {
            Some((Token::IDENT(s), end_span)) => Self::spanned(
                AstNode::ModuleStmt { name: s.to_string() },
                start.merge(end_span),
            ),
            Some((other, span)) => err_node!(self, span, "Expected identifier after 'namespace', got {:?}", other),
            None                => Self::spanned(AstNode::Error, start),
        }
    }

    fn parse_use(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let (first_tok, first_span) = match self.expect_any_some() {
            Some(t) => t,
            None    => return Self::spanned(AstNode::Error, start),
        };

        let mut base_path: SAst = match first_tok {
            Token::IDENT(s) => Self::spanned(AstNode::Identifier(s.to_string()), first_span),
            other => err_node!(self, first_span, "Expected identifier at start of use path, got {:?}", other),
        };

        let used = loop {
            match self.peek_some() {
                Token::PUNCT(".") => {
                    self.consume();
                    match self.peek_some() {
                        Token::PAREN("{") => {
                            self.consume();
                            let mut used = Vec::new();
                            loop {
                                match self.peek_some() {
                                    Token::PAREN("}") => { self.consume(); break; }
                                    Token::PUNCT(",")  => { self.consume(); }
                                    Token::IDENT(_) => {
                                        let actual = match self.expect_any_some() {
                                            Some((Token::IDENT(s), _)) => s.to_string(),
                                            _ => break,
                                        };
                                        let alias = if let Token::BINOP("::") = self.peek_some() {
                                            self.consume();
                                            match self.expect_any_some() {
                                                Some((Token::IDENT(s), _)) => s.to_string(),
                                                Some((other, span)) => {
                                                    self.push_error(
                                                        format!("Expected ident after 'as', got {:?}", other),
                                                        span,
                                                    );
                                                    break;
                                                }
                                                None => break,
                                            }
                                        } else {
                                            actual.clone()
                                        };
                                        used.push((actual, alias));
                                    }
                                    other => {
                                        let span = self.peek_span();
                                        self.push_error(
                                            format!("Expected ident or '}}' in use list, got {:?}", other),
                                            span,
                                        );
                                        break;
                                    }
                                }
                            }
                            break used;
                        }

                        Token::IDENT(_) => {
                            let (seg_tok, seg_span) = match self.expect_any_some() {
                                Some(t) => t,
                                None    => return Self::spanned(AstNode::Error, start),
                            };
                            let name = match seg_tok {
                                Token::IDENT(s) => s.to_string(),
                                _ => unreachable!(),
                            };
                            match self.peek_some() {
                                Token::PUNCT(".") => {
                                    let merged = base_path.span.merge(seg_span);
                                    base_path = Self::spanned(
                                        AstNode::DotIndex {
                                            indexee: Box::new(base_path),
                                            index:   Box::new(Self::spanned(
                                                AstNode::Identifier(name), seg_span,
                                            )),
                                        },
                                        merged,
                                    );
                                }
                                Token::BINOP("::") => {
                                    self.consume();
                                    let alias = match self.expect_any_some() {
                                        Some((Token::IDENT(s), _)) => s.to_string(),
                                        Some((other, span)) => {
                                            self.push_error(
                                                format!("Expected ident after 'as', got {:?}", other),
                                                span,
                                            );
                                            return Self::spanned(AstNode::Error, start);
                                        }
                                        None => return Self::spanned(AstNode::Error, start),
                                    };
                                    break vec![(name, alias)];
                                }
                                _ => break vec![(name.clone(), name)],
                            }
                        }

                        other => {
                            let span = self.peek_span();
                            err_node!(self, span, "Expected ident or '{{' after '::', got {:?}", other);
                        }
                    }
                }
                _ => break vec![],
            }
        };

        let span = start.merge(base_path.span);
        Self::spanned(AstNode::UseStmt { base_path: Box::new(base_path), used }, span)
    }
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_ret(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        let value = match self.peek_some() {
            Token::PAREN("}") | Token::END => None,
            _ => Some(Box::new(self.parse_expr(ctx))),
        };
        let span = value.as_ref().map(|v| start.merge(v.span)).unwrap_or(start);
        Self::spanned(AstNode::ReturnStmt { value }, span)
    }

    fn parse_for_loop(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        if self.expect_some(Token::PAREN("("), "Expected '(' after for").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let binding = match self.parse_binding(ctx) {
            Some(b) => b,
            None    => return Self::spanned(AstNode::Error, start),
        };
        if self.expect_some(Token::IN, "Expected 'in' after for-loop binding").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let iterator = self.parse_expr(ctx);
        if self.expect_some(Token::PAREN(")"), "Expected ')' after for loop head").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let body = self.parse_body(ctx);
        let end  = body.last().map(|s| s.span).unwrap_or(start);
        Self::spanned(
            AstNode::ForLoop { binding, iterator: Box::new(iterator), body },
            start.merge(end),
        )
    }

    fn parse_while_loop(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        if self.expect_some(Token::PAREN("("), "Expected '(' after while").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let condition = self.parse_expr(ctx);
        if self.expect_some(Token::PAREN(")"), "Expected ')' after while condition").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let body = self.parse_body(ctx);
        let end  = body.last().map(|s| s.span).unwrap_or(start);
        Self::spanned(AstNode::WhileLoop { condition: Box::new(condition), body }, start.merge(end))
    }

    fn parse_conditional_branch(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        if self.expect_some(Token::PAREN("("), "Expected '(' after if").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let condition = self.parse_expr(ctx);
        if self.expect_some(Token::PAREN(")"), "Expected ')' after if condition").is_none() {
            return Self::spanned(AstNode::Error, start);
        }

        let mut body:     Vec<SAst>        = Vec::new();
        let mut next:     Option<Box<SAst>> = None;
        let mut end_span                    = start;

        loop {
            match self.peek_some() {
                Token::END => {self.consume_end(&mut end_span); break;}
                Token::ELSE => {
                    let else_start = self.peek_span();
                    self.consume();
                    let mut else_body = Vec::new();
                    loop {
                        match self.peek_some() {
                            Token::END => {
                                if let Some((_, s)) = self.consume()
                                {
                                    end_span = s;
                                }
                                else
                                {
                                    self.push_error("No token".into(), end_span);
                                }
                                break;
                            }
                            _ => else_body.push(self.parse_stmt(ctx)),
                        }
                    }
                    let else_end = else_body.last().map(|s| s.span).unwrap_or(else_start);
                    next = Some(Box::new(Self::spanned(
                        AstNode::ConditionalBranch { condition: None, body: else_body, next: None },
                        else_start.merge(else_end),
                    )));
                    break;
                }
                Token::ELSEIF => {
                    self.consume();
                    let branch = self.parse_conditional_branch(ctx);
                    end_span = branch.span;
                    next = Some(Box::new(branch));
                    break;
                }
                _ => body.push(self.parse_stmt(ctx)),
            }
        }

        Self::spanned(
            AstNode::ConditionalBranch { condition: Some(Box::new(condition)), body, next },
            start.merge(end_span),
        )
    }

    fn parse_match_stmt(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        if self.expect_some(Token::PAREN("("), "Expected '(' after match").is_none() {
            return Self::spanned(AstNode::Error, start);
        }
        let matchee = self.parse_expr(ctx);
        if self.expect_some(Token::PAREN(")"), "Expected ')' after match head").is_none() {
            return Self::spanned(AstNode::Error, start);
        }

        let mut arms     = Vec::new();
        let mut end_span = start;

        loop {
            match self.peek_some() {
                Token::END => {self.consume_end(&mut end_span); break;}
                Token::PUNCT(",") => { self.consume(); }
                _ => {
                    let pattern = self.parse_match_pattern(ctx);

                    let body = match self.peek_some() {
                        Token::FATARROW => {
                            self.consume();
                            match self.peek_some() {
                                Token::PAREN("{") => {
                                    self.consume();
                                    let mut stmts = Vec::new();
                                    loop {
                                        match self.peek_some() {
                                            Token::PAREN("}") => { self.consume(); break; }
                                            _ => stmts.push(self.parse_stmt(ctx)),
                                        }
                                    }
                                    stmts
                                }
                                _ => vec![self.parse_stmt(ctx)],
                            }
                        }
                        Token::DO => {
                            self.consume();
                            let mut stmts = Vec::new();
                            loop {
                                match self.peek_some() {
                                    Token::END => { self.consume(); break; }
                                    _ => stmts.push(self.parse_stmt(ctx)),
                                }
                            }
                            stmts
                        }
                        other => {
                            let span = self.peek_span();
                            err_node!(
                                self, span,
                                "Expected '=>' or 'do' after match pattern, got {:?}", other
                            );
                        }
                    };

                    if let Some(last) = body.last() { end_span = last.span; }
                    arms.push(MatchArm { pattern, body });
                }
            }
        }

        Self::spanned(AstNode::MatchStmt { matchee: Box::new(matchee), arms }, start.merge(end_span))
    }

    fn parse_match_pattern(&mut self, ctx: &ParsingContext) -> MatchPattern {
        match self.peek_some() {
            Token::IDENT("_") => {
                self.consume();
                MatchPattern::Wildcard
            }

            Token::DEFAULT => {
                self.consume();
                let name = if let Token::IDENT(_) = self.peek_some() {
                    if let Some((t, ..)) = self.consume()
                    {
                        match t {
                            Token::IDENT(s) => Some(s.to_string()),
                            _ => unreachable!(),
                        }
                    }
                    else
                    {
                        let s = self.peek_span();
                        self.push_error("No token".into(), s);
                        None
                    }
                } else {
                    None
                };
                MatchPattern::Default(name)
            }

            Token::PAREN("(") => MatchPattern::Binding(
                self.parse_binding(ctx).unwrap_or(BindingNode::OrderedBinding { bindings: vec![] })
            ),

            Token::PAREN("{") => MatchPattern::Binding(
                self.parse_binding(ctx).unwrap_or(BindingNode::UnorderedBinding { bindings: vec![] })
            ),

            Token::IDENT(_) => {
                if let Some((first_tok, _)) = self.consume()
                {
                    let first = match first_tok {
                        Token::IDENT(s) => s.to_string(),
                        _ => unreachable!(),
                    };

                    if let Token::PUNCT(".") = self.peek_some() {
                        let mut path = vec![first];
                        while let Token::PUNCT(".") = self.peek_some() {
                            self.consume();
                            match self.expect_any_some() {
                                Some((Token::IDENT(s), _)) => path.push(s.to_string()),
                                Some((other, span)) => {
                                    self.push_error(
                                        format!("Expected variant name after '::', got {:?}", other),
                                        span,
                                    );
                                    break;
                                }
                                None => break,
                            }
                        }
                        let binding = if let Token::PAREN("(") = self.peek_some() {
                            self.parse_binding(ctx)
                        } else {
                            None
                        };
                        MatchPattern::CategoryVariant { path, binding }
                    } else {
                        MatchPattern::Binding(
                            BindingNode::IdentifierBinding { name: first, ty: TypeNode::Inferred }
                        )
                    }
                }
                else
                {
                    MatchPattern::Wildcard
                }
            }

            _ => MatchPattern::Expr(self.parse_expr(ctx)),
        }
    }
}

// ---------------------------------------------------------------------------
// Variable / function declarations
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_var_declaration(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();
        let binding = match self.parse_binding(ctx) {
            Some(b) => b,
            None    => return Self::spanned(AstNode::Error, start),
        };
        let init_value = if let Token::BINOP("=") = self.peek_some() {
            self.consume();
            Some(Box::new(self.parse_expr(ctx)))
        } else {
            None
        };
        let end = init_value.as_ref().map(|v| v.span).unwrap_or(start);
        Self::spanned(AstNode::VarDeclaration { binding, init_value }, start.merge(end))
    }

    fn parse_fun_declaration(&mut self, ctx: &ParsingContext) -> SAst {
        let start = self.peek_span();

        let name = match self.expect_any_some() {
            Some((Token::IDENT(s), _)) => s.to_string(),
            Some((other, span))        => err_node!(self, span, "Expected function name, got {:?}", other),
            None                       => return Self::spanned(AstNode::Error, start),
        };

        let mut type_params = Vec::new();
        if let Token::BINOP("<") = self.peek_some() {
            self.consume();
            loop {
                match self.peek_some() {
                    Token::BINOP(">") => { self.consume(); break; }
                    Token::PUNCT(",") => { self.consume(); }
                    _ => {
                        let node = match self.parse_type(ctx) {
                            Some(t) => t,
                            None    => return Self::spanned(AstNode::Error, start),
                        };
                        let constraint = if let Token::PUNCT(":") = self.peek_some() {
                            self.consume();
                            match self.parse_type(ctx) {
                                Some(t) => Some(t),
                                None    => return Self::spanned(AstNode::Error, start),
                            }
                        } else {
                            None
                        };
                        type_params.push((node, constraint));
                    }
                }
            }
        }

        if self.expect_some(Token::PAREN("("), "Expected '(' after function name").is_none() {
            return Self::spanned(AstNode::Error, start);
        }

        let mut params = Vec::new();
        loop {
            match self.peek_some() {
                Token::PAREN(")") => { self.consume(); break; }
                Token::PUNCT(",") => { self.consume(); }
                _ => match self.parse_binding(ctx) {
                    Some(b) => params.push(b),
                    None    => return Self::spanned(AstNode::Error, start),
                },
            }
        }

        let return_type = if let Token::PUNCT(":") = self.peek_some() {
            self.consume();
            match self.parse_type(ctx) {
                Some(t) => t,
                None    => return Self::spanned(AstNode::Error, start),
            }
        } else {
            TypeNode::Inferred
        };

        let body = if ctx.no_fn_body { Vec::new() } else { self.parse_body(ctx) };
        let end  = body.last().map(|s| s.span).unwrap_or(start);

        Self::spanned(
            AstNode::FunctionDeclaration { name, type_params, params, return_type, body },
            start.merge(end),
        )
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl LucyParser {
    fn parse_expr(&mut self, ctx: &ParsingContext) -> SAst {
        self.parse_expr_bp(ctx, 0)
    }

    fn get_bp(op: &Token) -> Option<u8> {
        match op {
            Token::BINOP("=")                           => Some(1),
            Token::BINOP("||")                          => Some(2),
            Token::BINOP("&&")                          => Some(3),
            Token::BINOP("|")                           => Some(4),
            Token::AND                                  => Some(5),
            Token::BINOP("==") | Token::BINOP("!=")
            | Token::BINOP("<") | Token::BINOP(">")
            | Token::BINOP("<=") | Token::BINOP(">=")  => Some(6),
            Token::BINOP("<<") | Token::BINOP(">>")    => Some(8),
            Token::BINOP("+")  | Token::BINOP("-")     => Some(10),
            Token::BINOP("*")  | Token::BINOP("/")
            | Token::BINOP("%")                        => Some(20),
            Token::BINOP("^")                          => Some(25),
            Token::BINOP("::")                                  => Some(30),
            _ => None,
        }
    }

    fn parse_expr_bp(&mut self, ctx: &ParsingContext, min_bp: u8) -> SAst {
        let mut left = self.parse_primary(ctx);
        left = self.parse_postfix(ctx, left);

        loop {
            let bp = match self.peek_tok() {
                Some(op) => match Self::get_bp(op) {
                    Some(bp) if bp > min_bp => bp,
                    _ => break,
                },
                None => break,
            };

            if let Some((op_token, op_span)) = self.consume()
            {
                match op_token {
                    Token::BINOP("::") => {
                        let ty = match self.parse_type(ctx) {
                            Some(t) => t,
                            None    => break,
                        };
                        let span = left.span.merge(op_span);
                        left = Self::spanned(AstNode::TypeCast { left: Box::new(left), right: ty }, span);
                    }
                    Token::BINOP("=") => {
                        let right = self.parse_expr_bp(ctx, 0);
                        let span  = left.span.merge(right.span);
                        left = Self::spanned(
                            AstNode::Assignment { left: Box::new(left), right: Box::new(right) },
                            span,
                        );
                    }
                    _ => {
                        let op = match op_token {
                            Token::BINOP("+")  => Operator::Add,
                            Token::BINOP("-")  => Operator::Sub,
                            Token::BINOP("*")  => Operator::Mul,
                            Token::BINOP("/")  => Operator::Div,
                            Token::BINOP("%")  => Operator::Mod,
                            Token::BINOP("^")  => Operator::Pow,
                            Token::BINOP("<<") => Operator::BLShift,
                            Token::BINOP(">>") => Operator::BRShift,
                            Token::BINOP("&&") => Operator::LAnd,
                            Token::BINOP("||") => Operator::LOr,
                            Token::AND         => Operator::BAnd,
                            Token::BINOP("|")  => Operator::BOr,
                            Token::BINOP("==") => Operator::Eq,
                            Token::BINOP("!=") => Operator::NEq,
                            Token::BINOP("<")  => Operator::Lt,
                            Token::BINOP(">")  => Operator::Gt,
                            Token::BINOP("<=") => Operator::Le,
                            Token::BINOP(">=") => Operator::Ge,
                            other => {
                                self.push_error(format!("Unknown binary operator: {:?}", other), op_span);
                                break;
                            }
                        };
                        let right = self.parse_expr_bp(ctx, bp);
                        let span  = left.span.merge(right.span);
                        let bin   = Self::spanned(
                            AstNode::BinaryOperation { op, left: Box::new(left), right: Box::new(right) },
                            span,
                        );
                        left = self.parse_postfix(ctx, bin);
                    }
                }
            }
            else
            {
                ()
            }
        }
        left
    }

    fn parse_postfix(&mut self, ctx: &ParsingContext, mut left: SAst) -> SAst {
        loop {
            left = match self.peek_tok() {
                Some(Token::PAREN("(")) => {
                    let open_span = self.peek_span();
                    self.consume();
                    let args = self.parse_call_args(ctx);
                    let end  = args.last().map(|a| a.span).unwrap_or(open_span);
                    let span = left.span.merge(end);
                    Self::spanned(AstNode::FunctionCall { callee: Box::new(left), args }, span)
                }

                Some(Token::PAREN("[")) => {
                    self.consume();
                    let index = self.parse_expr(ctx);
                    let close_span = match self.expect_some(Token::PAREN("]"), "Expected ']'") {
                        Some((_, s)) => s,
                        None         => break,
                    };
                    let span = left.span.merge(close_span);
                    Self::spanned(
                        AstNode::ComputedIndex { indexee: Box::new(left), index: Box::new(index) },
                        span,
                    )
                }

                Some(Token::PUNCT(":")) => {
                    self.consume();
                    match self.peek_some() {
                        Token::IDENT(_) => {
                            let (seg_tok, seg_span) = match self.expect_any_some() {
                                Some(t) => t,
                                None    => break,
                            };
                            let seg = match seg_tok {
                                Token::IDENT(s) => Self::spanned(AstNode::Identifier(s.to_string()), seg_span),
                                _ => unreachable!(),
                            };
                            let span = left.span.merge(seg_span);
                            Self::spanned(
                                AstNode::MethodCall { indexee: Box::new(left), index: Box::new(seg) },
                                span,
                            )
                        }
                        other => {
                            let span = self.peek_span();
                            self.push_error(
                                format!("Expected ident after ':', got {:?}", other),
                                span,
                            );
                            break;
                        }
                    }
                }

                Some(Token::PUNCT(".")) => {
                    self.consume();
                    match self.peek_some() {
                        Token::BINOP("<") => {
                            self.consume();
                            let mut type_args = Vec::new();
                            loop {
                                match self.peek_some() {
                                    Token::BINOP(">") => { self.consume(); break; }
                                    Token::PUNCT(",")  => { self.consume(); }
                                    _ => match self.parse_type(ctx) {
                                        Some(t) => type_args.push(t),
                                        None    => break,
                                    },
                                }
                            }
                            let span = left.span;
                            Self::spanned(
                                AstNode::TypeInstantiation { callee: Box::new(left), type_args },
                                span,
                            )
                        }
                        Token::IDENT(_) => {
                            let (seg_tok, seg_span) = match self.expect_any_some() {
                                Some(t) => t,
                                None    => break,
                            };
                            let seg = match seg_tok {
                                Token::IDENT(s) => Self::spanned(AstNode::Identifier(s.to_string()), seg_span),
                                _ => unreachable!(),
                            };
                            let span = left.span.merge(seg_span);
                            Self::spanned(
                                AstNode::DotIndex { indexee: Box::new(left), index: Box::new(seg) },
                                span,
                            )
                        }
                        other => {
                            let span = self.peek_span();
                            self.push_error(
                                format!("Expected ident or '<' after '::', got {:?}", other),
                                span,
                            );
                            break;
                        }
                    }
                }

                Some(Token::PAREN("{")) => self.parse_class_literal(ctx, left),

                _ => break,
            };
        }
        left
    }

    fn parse_class_literal(&mut self, ctx: &ParsingContext, ty: SAst) -> SAst {
        let start = ty.span;
        if self.expect_some(Token::PAREN("{"), "Expected '{'").is_none() {
            return Self::spanned(AstNode::Error, start);
        }

        let mut fields   = Vec::new();
        let mut end_span = start;

        loop {
            match self.peek_some() {
                Token::PAREN("}") => {self.consume_end(&mut end_span); break;}
                Token::PUNCT(",") => { self.consume(); }
                Token::IDENT(_) => {
                    let name = match self.expect_any_some() {
                        Some((Token::IDENT(s), _)) => s.to_string(),
                        _ => break,
                    };
                    if self.expect_some(Token::BINOP("="), "Expected '=' after field name").is_none() {
                        break;
                    }
                    let value = self.parse_expr(ctx);
                    end_span  = value.span;
                    fields.push((name, value));
                }
                other => {
                    let span = self.peek_span();
                    self.push_error(
                        format!("Expected field or '}}' in struct literal, got {:?}", other),
                        span,
                    );
                    break;
                }
            }
        }

        Self::spanned(AstNode::ClassLiteral { ty: Box::new(ty), fields }, start.merge(end_span))
    }

    fn parse_primary(&mut self, ctx: &ParsingContext) -> SAst {
        let (tok, span) = match self.expect_any_some() {
            Some(t) => t,
            None    => return Self::spanned(AstNode::Error, Span::dummy()),
        };

        match tok {
            Token::INT(n)    => Self::spanned(AstNode::IntLiteral(n),                span),
            Token::FLOAT(f)  => Self::spanned(AstNode::FloatLiteral(f),              span),
            Token::STRING(s) => Self::spanned(AstNode::StringLiteral(s.to_string()), span),

            Token::IDENT(s) => {
                let name = s.to_string();
                if let Token::BANG = self.peek_some() {
                    self.parse_macro_invocation(name, span)
                } else {
                    Self::spanned(AstNode::Identifier(name), span)
                }
            }

            Token::SELF => Self::spanned(AstNode::SelfExpr, span),

            Token::SELFTYPE => {
                match &ctx.current_class {
                    Some(name) => Self::spanned(AstNode::Identifier(name.clone()), span),
                    None => {
                        self.push_error("'Self' used outside a class body".to_string(), span);
                        Self::spanned(AstNode::Error, span)
                    }
                }
            }

            Token::TYPEOF => {
                let operand = self.parse_expr(ctx);
                let end     = operand.span;
                Self::spanned(AstNode::Typeof(Box::new(operand)), span.merge(end))
            }

            Token::AND => {
                let operand = self.parse_primary(ctx);
                let end     = operand.span;
                Self::spanned(AstNode::Borrowed(Box::new(operand)), span.merge(end))
            }

            Token::UNARY("-") => {
                let operand = self.parse_primary(ctx);
                let end     = operand.span;
                Self::spanned(
                    AstNode::UnaryOperation { op: Operator::Neg, right: Box::new(operand) },
                    span.merge(end),
                )
            }

            Token::BANG => {
                let operand = self.parse_primary(ctx);
                let end     = operand.span;
                Self::spanned(
                    AstNode::UnaryOperation { op: Operator::LNot, right: Box::new(operand) },
                    span.merge(end),
                )
            }

            Token::UNARY("~") => {
                let operand = self.parse_primary(ctx);
                let end     = operand.span;
                Self::spanned(
                    AstNode::UnaryOperation { op: Operator::BNot, right: Box::new(operand) },
                    span.merge(end),
                )
            }

            Token::PAREN("[") => {
                let mut members  = Vec::new();
                let mut end_span = span;
                loop {
                    match self.peek_some() {
                        Token::PAREN("]") => {self.consume_end(&mut end_span); break;}
                        Token::PUNCT(",") => { self.consume(); }
                        _                 => members.push(self.parse_expr(ctx)),
                    }
                }
                Self::spanned(AstNode::ArrayLiteral(members), span.merge(end_span))
            }

            Token::PAREN("{") => {
                let mut pairs    = Vec::new();
                let mut end_span = span;
                loop {
                    match self.peek_some() {
                        Token::PAREN("}") => {self.consume_end(&mut end_span); break;}
                        Token::PUNCT(",") => { self.consume(); }
                        _ => {
                            let key = self.parse_expr(ctx);
                            if self.expect_some(Token::PUNCT(":"), "Expected ':' after hashmap key").is_none() {
                                break;
                            }
                            let value = self.parse_expr(ctx);
                            end_span  = value.span;
                            pairs.push((key, value));
                        }
                    }
                }
                Self::spanned(AstNode::HashMapLiteral(pairs), span.merge(end_span))
            }

            Token::ARROW => {
                let operand = self.parse_primary(ctx);
                let end     = operand.span;
                Self::spanned(AstNode::Moved(Box::new(operand)), span.merge(end))
            }

            Token::FSTRING_START => {
                let mut parts    = Vec::new();
                let mut end_span = span;
                loop {
                    match self.peek_some() {
                        Token::FSTRING_END => self.consume_end(&mut end_span),
                        Token::FSTRING_CHUNK(s) => {
                            self.consume();
                            parts.push(FmtPart::Literal(s.to_string()));
                        }
                        Token::FSTRING_EXPR_START => {
                            self.consume();
                            let expr = self.parse_expr(ctx);
                            if self.expect_some(
                                Token::FSTRING_EXPR_END,
                                "Expected '}}' after fstring expression",
                            ).is_none() {
                                break;
                            }
                            parts.push(FmtPart::Expr(expr));
                        }
                        other => {
                            let span = self.peek_span();
                            self.push_error(
                                format!("Unexpected token inside fstring: {:?}", other),
                                span,
                            );
                            break;
                        }
                    }
                }
                Self::spanned(AstNode::FmtStringLiteral(parts), span.merge(end_span))
            }

            Token::PAREN("(") => {
                let expr = self.parse_expr(ctx);
                let close = match self.expect_some(Token::PAREN(")"), "Expected ')'") {
                    Some((_, s)) => s,
                    None         => expr.span,
                };
                Self::spanned(expr.node, span.merge(close))
            }

            other => {
                self.push_error(format!("Unexpected token in expression: {:?}", other), span);
                Self::spanned(AstNode::Error, span)
            }
        }
    }
}