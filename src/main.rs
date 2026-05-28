use lucy_lang::ty::{FunctionType, Type};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use lucy_lang::lexer::{tokenize, Token};
use lucy_lang::parser::LucyParser;
use lucy_lang::typechecker::{Namespace, TypeChecker};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

struct Backend {
    client: Client,
    docs: Arc<Mutex<HashMap<Url, String>>>,
}

fn word_at_position(
    src: &str,
    pos: Position,
) -> Option<String> {
    let lines: Vec<&str> =
        src.lines().collect();

    let line =
        lines.get(pos.line as usize)?;

    let chars: Vec<char> =
        line.chars().collect();

    let mut start =
        pos.character as usize;

    while start > 0
        && (chars[start - 1].is_alphanumeric()
            || chars[start - 1] == '_')
    {
        start -= 1;
    }

    let mut end =
        pos.character as usize;

    while end < chars.len()
        && (chars[end].is_alphanumeric()
            || chars[end] == '_')
    {
        end += 1;
    }

    Some(
        chars[start..end]
            .iter()
            .collect()
    )
}

fn semantic_token_type(tok: &Token) -> Option<u32> {
    match tok {
        Token::MODULE
        | Token::USE
        | Token::PUB
        | Token::CLASS
        | Token::FN
        | Token::FOR
        | Token::IN
        | Token::RETURN
        | Token::END
        | Token::AS
        | Token::SELF
        | Token::SELFTYPE
        | Token::IF
        | Token::ELSE
        | Token::ELSEIF
        | Token::WHILE
        | Token::MATCH
        | Token::DECLARE
        | Token::MUTABLE
        | Token::OPERATOR
        | Token::TYPEOF => Some(0),

        Token::STRING(_) => Some(4),

        Token::INT(_) | Token::FLOAT(_) => Some(5),

        Token::IDENT(name) => {
            match *name {
                "string"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "bool"
                | "usize"
                | "Self" => Some(6),

                _ => Some(3),
            }
        }

        _ => None,
    }
}

fn semantic_tokens(src: &str) -> Vec<SemanticToken> {
    let tokens = tokenize(src.to_string());

    let mut out = Vec::new();

    let mut prev_line = 0u32;
    let mut prev_col = 0u32;

    for i in 0..tokens.len() {
        let (tok, span) = &tokens[i];

        let token_type = match tok {
            Token::MODULE
            | Token::USE
            | Token::PUB
            | Token::CLASS
            | Token::FN
            | Token::FOR
            | Token::IN
            | Token::RETURN
            | Token::END
            | Token::AS
            | Token::SELF
            | Token::SELFTYPE
            | Token::IF
            | Token::ELSE
            | Token::ELSEIF
            | Token::WHILE
            | Token::MATCH
            | Token::DECLARE
            | Token::MUTABLE
            | Token::OPERATOR
            | Token::TYPEOF => Some(0),

            Token::STRING(_) => Some(4),

            Token::INT(_)
            | Token::FLOAT(_) => Some(5),

            Token::IDENT(name) => {
                match *name {
                    "string"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "bool"
                    | "usize"
                    | "Self" => Some(6),

                    _ => {
                        let prev =
                            if i > 0 {
                                Some(&tokens[i - 1].0)
                            } else {
                                None
                            };

                        let next =
                            tokens.get(i + 1)
                                .map(|t| &t.0);

                        match prev {
                            Some(Token::MODULE) => Some(7),

                            Some(Token::CLASS) => Some(2),

                            Some(Token::USE) => Some(7),

                            _ => {
                                match next {
                                    Some(Token::PUNCT("."))
                                    | Some(Token::PUNCT(":")) => Some(7),

                                    _ => {
                                        if name.chars().next()
                                            .map(|c| c.is_uppercase())
                                            .unwrap_or(false)
                                        {
                                            Some(2)
                                        } else {
                                            Some(3)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            _ => None,
        };

        let Some(token_type) = token_type else {
            continue;
        };

        let (line, col) =
            span_to_line_col(src, span.start);

        let len =
            (span.end - span.start) as u32;

        let delta_line =
            line - prev_line;

        let delta_start =
            if delta_line == 0 {
                col - prev_col
            } else {
                col
            };

        out.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type,
            token_modifiers_bitset: 0,
        });

        prev_line = line;
        prev_col = col;
    }

    out
}

fn span_to_line_col(
    src: &str,
    offset: usize,
) -> (u32, u32) {
    let up_to = &src[..offset.min(src.len())];

    let line =
        up_to.bytes()
            .filter(|&b| b == b'\n')
            .count() as u32;

    let col =
        up_to
            .rfind('\n')
            .map(|p| offset - p - 1)
            .unwrap_or(offset) as u32;

    (line, col)
}

fn parse_and_collect_errors(
    src: &str,
) -> Vec<Diagnostic> {
    let tokens = tokenize(src.to_string());

    let mut parser =
        LucyParser::new(tokens);

    let ast =
        parser.parse_file_source();

    let mut diagnostics =
        Vec::<Diagnostic>::new();

    if !parser.errors.is_empty() {
        diagnostics.extend(
            parser
                .errors
                .into_iter()
                .map(|error| {
                    let span = error.span;

                    let (start_line, start_col) =
                        span_to_line_col(
                            src,
                            span.start,
                        );

                    let (end_line, end_col) =
                        span_to_line_col(
                            src,
                            span.end,
                        );

                    Diagnostic {
                        range: Range {
                            start: Position {
                                line: start_line,
                                character: start_col,
                            },

                            end: Position {
                                line: end_line,
                                character: end_col,
                            },
                        },

                        severity: Some(
                            DiagnosticSeverity::ERROR
                        ),

                        source: Some(
                            "lucy-parser".into()
                        ),

                        message: error.message,

                        ..Default::default()
                    }
                })
        );

        return diagnostics;
    }

    let mut stdio =
        Namespace::new();

    stdio.locals.insert(
        "println".into(),
        Type::Function(Box::new(
            FunctionType {
                params: vec![Type::Unknown],
                return_type: Box::new(Type::Empty),
            }
        )),
    );

    let mut checker = TypeChecker::new();

    checker.scopes.define_namespace(
        "stdio".into(),
        stdio,
    );

    checker.check_program(&ast);

    diagnostics.extend(
        checker
            .errors
            .into_iter()
            .map(|error| {
                let span = error.span;

                let (start_line, start_col) =
                    span_to_line_col(
                        src,
                        span.start,
                    );

                let (end_line, end_col) =
                    span_to_line_col(
                        src,
                        span.end,
                    );

                Diagnostic {
                    range: Range {
                        start: Position {
                            line: start_line,
                            character: start_col,
                        },

                        end: Position {
                            line: end_line,
                            character: end_col,
                        },
                    },

                    severity: Some(
                        DiagnosticSeverity::ERROR
                    ),

                    source: Some(
                        "lucy-typechecker".into()
                    ),

                    message: error.message,

                    ..Default::default()
                }
            })
    );

    diagnostics
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(
        &self,
        _: InitializeParams,
    ) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync:
                    Some(
                        TextDocumentSyncCapability::Kind(
                            TextDocumentSyncKind::FULL,
                        )
                    ),

                hover_provider:
                    Some(
                        HoverProviderCapability::Simple(
                            true
                        )
                    ),

                completion_provider:
                    Some(
                        CompletionOptions {
                            trigger_characters:
                                Some(vec![
                                    ".".into(),
                                    ":".into(),
                                ]),

                            ..Default::default()
                        }
                    ),

                semantic_tokens_provider:
                    Some(
                        SemanticTokensServerCapabilities::SemanticTokensOptions(
                            SemanticTokensOptions {
                                legend: SemanticTokensLegend {
                                    token_types: vec![
                                        SemanticTokenType::KEYWORD,   // 0
                                        SemanticTokenType::FUNCTION,  // 1
                                        SemanticTokenType::CLASS,     // 2
                                        SemanticTokenType::VARIABLE,  // 3
                                        SemanticTokenType::STRING,    // 4
                                        SemanticTokenType::NUMBER,    // 5
                                        SemanticTokenType::TYPE,      // 6
                                        SemanticTokenType::NAMESPACE, // 7
                                    ],

                                    token_modifiers: vec![],
                                },

                                full: Some(
                                    SemanticTokensFullOptions::Bool(
                                        true
                                    )
                                ),

                                range: None,

                                work_done_progress_options:
                                    Default::default(),
                            }
                        )
                    ),

                ..Default::default()
            },

            server_info: Some(ServerInfo {
                name: "lucy-lsp".into(),
                version: Some("0.1.0".into()),
            }),
        })
    }

    async fn initialized(
        &self,
        _: InitializedParams,
    ) {
        self.client
            .log_message(
                MessageType::INFO,
                "lucy-lsp initialized",
            )
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(
        &self,
        params: DidOpenTextDocumentParams,
    ) {
        let src =
            params.text_document.text.clone();

        self.docs
            .lock()
            .await
            .insert(
                params.text_document.uri.clone(),
                src.clone(),
            );

        let diags =
            parse_and_collect_errors(&src);

        self.client
            .publish_diagnostics(
                params.text_document.uri,
                diags,
                None,
            )
            .await;
    }

    async fn did_change(
        &self,
        params: DidChangeTextDocumentParams,
    ) {
        if let Some(change) =
            params.content_changes.into_iter().last()
        {
            let src = change.text.clone();

            self.docs
                .lock()
                .await
                .insert(
                    params.text_document.uri.clone(),
                    src.clone(),
                );

            let diags =
                parse_and_collect_errors(&src);

            self.client
                .publish_diagnostics(
                    params.text_document.uri,
                    diags,
                    None,
                )
                .await;
        }
    }

    async fn hover(
        &self,
        params: HoverParams,
    ) -> Result<Option<Hover>> {
        let td =
            params.text_document_position_params;

        let docs =
            self.docs.lock().await;

        let Some(src) =
            docs.get(&td.text_document.uri)
        else {
            return Ok(None);
        };

        let Some(word) =
            word_at_position(src, td.position)
        else {
            return Ok(None);
        };

        Ok(Some(Hover {
            contents:
                HoverContents::Markup(
                    MarkupContent {
                        kind: MarkupKind::Markdown,

                        value: format!(
                            "```lucy\n{}\n```",
                            word
                        ),
                    }
                ),

            range: None,
        }))
    }

    async fn completion(
        &self,
        _: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let keywords = [
            "var",
            "func",
            "return",
            "class",
            "if",
            "else",
            "elif",
            "while",
            "for",
            "do",
            "end",
            "in",
            "public",
            "namespace",
            "use",
            "match",
            "mut",
            "operator",
            "as",
            "self",
            "Self",
            "typeof",
            "throw",
        ];

        let items =
            keywords
                .iter()
                .map(|kw| CompletionItem {
                    label: kw.to_string(),

                    kind: Some(
                        CompletionItemKind::KEYWORD
                    ),

                    ..Default::default()
                })
                .collect();

        Ok(Some(
            CompletionResponse::Array(items)
        ))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let docs =
            self.docs.lock().await;

        let Some(src) =
            docs.get(&params.text_document.uri)
        else {
            return Ok(None);
        };

        Ok(Some(
            SemanticTokensResult::Tokens(
                SemanticTokens {
                    result_id: None,
                    data: semantic_tokens(src),
                }
            )
        ))
    }
}

#[tokio::main]
async fn main() {
    let stdin =
        tokio::io::stdin();

    let stdout =
        tokio::io::stdout();

    eprintln!("RUST: starting server");

    let (service, socket) =
        LspService::new(|client| Backend {
            client,

            docs: Arc::new(
                Mutex::new(HashMap::new())
            ),
        });

    Server::new(
        stdin,
        stdout,
        socket,
    )
    .serve(service)
    .await;

    eprintln!("RUST: server started");
}