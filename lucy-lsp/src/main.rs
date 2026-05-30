use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use lucy_lang::lexer::{tokenize, Token};
use lucy_lang::parser::{LucyParser, AstNode, BindingNode, SAst};
use lucy_lang::typechecker::{TypeChecker, Namespace as TcNamespace};
use lucy_lang::ty::{Type, FunctionType};
use lucy_lang::lib_std;
use lucy_lang::lucy_mod;

use std::collections::HashMap;
use std::sync::Arc;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

struct Backend {
    client: Client,
    docs:   Arc<Mutex<HashMap<Url, String>>>,
}

// ── Reuse same stub/load logic as lucy-run ───────────────────────────────────

fn stub_tc_namespace(ast: &SAst) -> TcNamespace {
    let mut ns = TcNamespace::new();
    let stmts = match &ast.node {
        AstNode::Program(s) => s,
        _ => return ns,
    };
    for stmt in stmts {
        let inner = match &stmt.node {
            AstNode::Public(i) => i.as_ref(),
            _ => continue,
        };
        match &inner.node {
            AstNode::FunctionDeclaration { name, params, return_type, .. } => {
                let fn_ty = FunctionType {
                    params: params.iter().map(|p| match p {
                        BindingNode::IdentifierBinding { ty, .. } =>
                            TypeChecker::compile_type_static(ty),
                        _ => Type::Unknown,
                    }).collect(),
                    return_type: Box::new(TypeChecker::compile_type_static(return_type)),
                };
                ns.locals.insert(name.clone(), Type::Function(Box::new(fn_ty)));
            }
            AstNode::ClassDefinition { name, .. } => {
                ns.types.insert(name.clone(), Type::TypeVar(name.clone()));
            }
            AstNode::VarDeclaration { binding, .. } => {
                if let BindingNode::IdentifierBinding { name, ty } = binding {
                    ns.locals.insert(name.clone(), TypeChecker::compile_type_static(ty));
                }
            }
            _ => {}
        }
    }
    ns
}

fn load_tc_registry(lib_path: &str) -> HashMap<String, TcNamespace> {
    let mut tc_registry: HashMap<String, TcNamespace> = HashMap::new();

    let stdio = lib_std::stdio_module();
    tc_registry.insert(stdio.name.to_string(), stdio.as_type_namespace());

    let lib = Path::new(lib_path);
    if !lib.exists() { return tc_registry; }

    let parsed: Vec<(String, SAst)> = std::fs::read_dir(lib)
        .expect("failed to read lib_path")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.extension().map(|x| x == "luc").unwrap_or(false)
                && path.file_stem().map(|s| s != "main").unwrap_or(false)
        })
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().to_string();
            let src  = std::fs::read_to_string(e.path()).ok()?;
            let tokens = tokenize(src);
            let mut parser = LucyParser::new(tokens);
            let ast = parser.parse_file_source();
            if !parser.errors.is_empty() { return None; }
            Some((name, ast))
        })
        .collect();

    // Pass 1 — stubs
    for (name, ast) in &parsed {
        tc_registry.insert(name.clone(), stub_tc_namespace(ast));
    }

    // Pass 2 — full typecheck
    for (name, ast) in &parsed {
        let mut checker = TypeChecker::new();
        checker.module_registry.modules = tc_registry.clone();
        checker.check_program(ast);

        let mut ns = TcNamespace::new();
        if let Some(top) = checker.scopes.scopes.first() {
            for (n, local) in &top.locals {
                ns.locals.insert(n.clone(), local.ty.clone());
            }
            for (n, ty) in &top.types {
                ns.types.insert(n.clone(), ty.clone());
            }
        }
        tc_registry.insert(name.clone(), ns);
    }

    tc_registry
}

// ── Find project root by walking up from the file looking for Lucy.luc ───────

fn find_project_root(file_path: &Path) -> Option<PathBuf> {
    let mut dir = file_path.parent()?;
    loop {
        if dir.join("Lucy.luc").exists() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn is_lucy_config_file(file_path: &Path) -> bool {
    file_path.file_name().map(|n| n == "Lucy.luc").unwrap_or(false)
}

// ── Diagnostics ──────────────────────────────────────────────────────────────

fn parse_and_collect_errors(src: &str, file_path: Option<&Path>) -> Vec<Diagnostic> {
    let tokens = tokenize(src.to_string());
    let mut parser = LucyParser::new(tokens);
    let ast = parser.parse_file_source();

    let mut diagnostics = Vec::<Diagnostic>::new();

    if !parser.errors.is_empty() {
        return parser.errors.into_iter().map(|e| {
            make_diagnostic(src, e.span, e.message, "lucy-parser")
        }).collect();
    }

    let mut checker = TypeChecker::new();

    // If this is Lucy.luc, pre-register the Lucy module (LucyConfig etc.)
    if file_path.map(is_lucy_config_file).unwrap_or(false) {
        let lucy_module = lucy_mod::lucy_config_module();
        lucy_module.register_into_typechecker(&mut checker);
    } else {
        // Normal file — load full project registry
        let lib_path = file_path
            .and_then(|p| find_project_root(p))
            .map(|root| {
                // Read lib_path from Lucy.luc if present, else default to root/src
                let lucy_luc = root.join("Lucy.luc");
                if lucy_luc.exists() {
                    lucy_mod::read_configs(root.to_str().unwrap_or(".").to_string())
                        .lib_path
                } else {
                    root.join("src").to_string_lossy().to_string()
                }
            })
            .unwrap_or_else(|| "./src".to_string());

        let tc_registry = load_tc_registry(&lib_path);
        checker.module_registry.modules = tc_registry;
    }

    checker.check_program(&ast);

    diagnostics.extend(checker.errors.into_iter().map(|e| {
        make_diagnostic(src, e.span, e.message, "lucy-typechecker")
    }));

    diagnostics
}

fn make_diagnostic(src: &str, span: lucy_lang::span::Span, message: String, source: &str) -> Diagnostic {
    let (start_line, start_col) = span_to_line_col(src, span.start);
    let (end_line,   end_col)   = span_to_line_col(src, span.end);
    Diagnostic {
        range: Range {
            start: Position { line: start_line, character: start_col },
            end:   Position { line: end_line,   character: end_col   },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source:   Some(source.into()),
        message,
        ..Default::default()
    }
}

// ── Helpers (unchanged) ───────────────────────────────────────────────────────

fn word_at_position(src: &str, pos: Position) -> Option<String> {
    let lines: Vec<&str> = src.lines().collect();
    let line = lines.get(pos.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let mut start = pos.character as usize;
    while start > 0 && (chars[start-1].is_alphanumeric() || chars[start-1] == '_') {
        start -= 1;
    }
    let mut end = pos.character as usize;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    Some(chars[start..end].iter().collect())
}

fn span_to_line_col(src: &str, offset: usize) -> (u32, u32) {
    let up_to = &src[..offset.min(src.len())];
    let line  = up_to.bytes().filter(|&b| b == b'\n').count() as u32;
    let col   = up_to.rfind('\n').map(|p| offset - p - 1).unwrap_or(offset) as u32;
    (line, col)
}

fn semantic_tokens(src: &str) -> Vec<SemanticToken> {
    let tokens = tokenize(src.to_string());
    let mut out = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_col  = 0u32;

    for i in 0..tokens.len() {
        let (tok, span) = &tokens[i];

        let token_type = match tok {
            Token::MODULE | Token::USE  | Token::PUB   | Token::CLASS
            | Token::FN   | Token::FOR  | Token::IN    | Token::RETURN
            | Token::END  | Token::AS   | Token::SELF  | Token::SELFTYPE
            | Token::IF   | Token::ELSE | Token::WHILE
            | Token::MATCH| Token::DECLARE | Token::MUTABLE
            | Token::OPERATOR | Token::TYPEOF | Token::GLOBAL | Token::DO => Some(0),

            Token::STRING(_) => Some(4),
            Token::INT(_) | Token::FLOAT(_) => Some(5),

            Token::IDENT(name) => match *name {
                "string"|"u8"|"u16"|"u32"|"u64"
                |"i8"|"i16"|"i32"|"i64"|"bool"|"usize"|"Self"|"int"|"float"|"double"|"char" => Some(6),
                _ => {
                    let prev = if i > 0 { Some(&tokens[i-1].0) } else { None };
                    let next = tokens.get(i+1).map(|t| &t.0);
                    match prev {
                        Some(Token::MODULE) | Some(Token::USE) => Some(7),
                        Some(Token::CLASS)                     => Some(2),
                        _ => match next {
                            Some(Token::PUNCT(".")) | Some(Token::PUNCT(":")) => Some(7),
                            _ => if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                                Some(2)
                            } else {
                                Some(3)
                            }
                        }
                    }
                }
            },
            _ => None,
        };

        let Some(token_type) = token_type else { continue; };
        let (line, col) = span_to_line_col(src, span.start);
        let len = (span.end - span.start) as u32;
        let delta_line  = line - prev_line;
        let delta_start = if delta_line == 0 { col - prev_col } else { col };

        out.push(SemanticToken {
            delta_line, delta_start, length: len, token_type, token_modifiers_bitset: 0,
        });
        prev_line = line;
        prev_col  = col;
    }
    out
}

// ── LSP impl (unchanged except did_open/did_change pass file path) ────────────

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider:     Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), ":".into()]),
                    ..Default::default()
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                        legend: SemanticTokensLegend {
                            token_types: vec![
                                SemanticTokenType::KEYWORD,
                                SemanticTokenType::FUNCTION,
                                SemanticTokenType::CLASS,
                                SemanticTokenType::VARIABLE,
                                SemanticTokenType::STRING,
                                SemanticTokenType::NUMBER,
                                SemanticTokenType::TYPE,
                                SemanticTokenType::NAMESPACE,
                            ],
                            token_modifiers: vec![],
                        },
                        full:  Some(SemanticTokensFullOptions::Bool(true)),
                        range: None,
                        work_done_progress_options: Default::default(),
                    })
                ),
                ..Default::default()
            },
            server_info: Some(ServerInfo { name: "lucy-lsp".into(), version: Some("0.1.0".into()) }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "lucy-lsp initialized").await;
    }

    async fn shutdown(&self) -> Result<()> { Ok(()) }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let src  = params.text_document.text.clone();
        let uri  = params.text_document.uri.clone();
        let path = uri.to_file_path().ok();

        self.docs.lock().await.insert(uri.clone(), src.clone());

        let diags = parse_and_collect_errors(&src, path.as_deref());
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            let src  = change.text.clone();
            let uri  = params.text_document.uri.clone();
            let path = uri.to_file_path().ok();

            self.docs.lock().await.insert(uri.clone(), src.clone());

            let diags = parse_and_collect_errors(&src, path.as_deref());
            self.client.publish_diagnostics(uri, diags, None).await;
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let td   = params.text_document_position_params;
        let docs = self.docs.lock().await;
        let Some(src)  = docs.get(&td.text_document.uri) else { return Ok(None); };
        let Some(word) = word_at_position(src, td.position) else { return Ok(None); };
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind:  MarkupKind::Markdown,
                value: format!("```lucy\n{}\n```", word),
            }),
            range: None,
        }))
    }

    async fn completion(&self, _: CompletionParams) -> Result<Option<CompletionResponse>> {
        let keywords = [
            "var","func","return","class","if","else","elif","while",
            "for","do","end","in","public","namespace","use","match",
            "mut","operator","as","self","Self","typeof","throw",
        ];
        let items = keywords.iter().map(|kw| CompletionItem {
            label: kw.to_string(),
            kind:  Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        }).collect();
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
        let docs = self.docs.lock().await;
        let Some(src) = docs.get(&params.text_document.uri) else { return Ok(None); };
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: semantic_tokens(src),
        })))
    }
}

#[tokio::main]
async fn main() {
    let stdin  = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    eprintln!("RUST: starting server");
    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Arc::new(Mutex::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
    eprintln!("RUST: server started");
}