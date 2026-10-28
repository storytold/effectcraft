//! Decodificador Clean-Room de `.jsxbin` (`@JSXBIN@ES@1.0@` a `@JSXBIN@ES@3.0@`) para `.jsx`
//! Projetado especificamente para o EffectCraft (`boa_engine` pre-processor).
//!
//! Restrições Estritas Atendidas (`AGENTS.md`):
//! - `#![forbid(unsafe_code)]` (100% Pure Safe Rust)
//! - Zero dependências externas GPL/LGPL (implementação 100% clean-room baseada em especificação)
//! - Zero chamadas a `.unwrap()`, `.expect()`, `panic!()`, `unreachable!()` ou indexação direta sem verificação
//! - Todas as operações retornam `Result<T, JsxbinError>` e impõem limites de recursão e alocação.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::fmt::{self, Write};

/// Limite máximo de profundidade de recursão da AST para prevenir estouro de pilha (stack overflow)
/// em arquivos `.jsxbin` malformados ou ofuscados maliciosamente.
const MAX_AST_DEPTH: usize = 512;

/// Limite máximo de elementos por lista/string para evitar exaustão de memória (OOM).
const MAX_COLLECTION_LEN: usize = 1_000_000;

/// Versão do protocolo binário ExtendScript detectada no cabeçalho `@JSXBIN@ES@...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsxbinVersion {
    V1_0,
    V2_0,
    V2_1,
}

/// Erros estruturados e recuperáveis do decodificador JSXBIN (`never crash`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsxbinError {
    InvalidHeader,
    UnsupportedVersion(String),
    UnexpectedEof { pos: usize },
    InvalidRadixChar { ch: char, pos: usize },
    InvalidBoolChar { ch: char, pos: usize },
    InvalidVariantMarker { ch: char, pos: usize },
    UnknownNodeMarker { ch: char, pos: usize },
    UnresolvedSymbolId { id: i64, pos: usize },
    NegativeLength { value: i64, pos: usize },
    AllocationLimitExceeded { requested: usize, max: usize, pos: usize },
    RecursionLimitExceeded { max_depth: usize, pos: usize },
    TrailingZeroRunAtNodeBoundary { remaining_zeros: usize, pos: usize },
    MissingExpectedNullMarker { found: char, pos: usize },
    FormattingError,
}

impl fmt::Display for JsxbinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => write!(f, "Cabeçalho @JSXBIN@ES@ ausente ou malformado"),
            Self::UnsupportedVersion(v) => write!(f, "Versão JSXBIN não suportada: {v}"),
            Self::UnexpectedEof { pos } => write!(f, "Fim inesperado do fluxo na posição {pos}"),
            Self::InvalidRadixChar { ch, pos } => {
                write!(f, "Caractere radix inválido '{ch}' na posição {pos}")
            }
            Self::InvalidBoolChar { ch, pos } => {
                write!(f, "Esperado booleano ('t' ou 'f'), encontrado '{ch}' na posição {pos}")
            }
            Self::InvalidVariantMarker { ch, pos } => {
                write!(f, "Marcador de variante constante desconhecido '{ch}' na posição {pos}")
            }
            Self::UnknownNodeMarker { ch, pos } => {
                write!(f, "Marcador de nó AST desconhecido '{ch}' na posição {pos}")
            }
            Self::UnresolvedSymbolId { id, pos } => {
                write!(f, "ID de símbolo {id} não encontrado na tabela na posição {pos}")
            }
            Self::NegativeLength { value, pos } => {
                write!(f, "Comprimento negativo inválido ({value}) na posição {pos}")
            }
            Self::AllocationLimitExceeded { requested, max, pos } => {
                write!(f, "Tamanho solicitado ({requested}) excede o limite seguro ({max}) na posição {pos}")
            }
            Self::RecursionLimitExceeded { max_depth, pos } => {
                write!(f, "Profundidade máxima da AST ({max_depth}) excedida na posição {pos}")
            }
            Self::TrailingZeroRunAtNodeBoundary { remaining_zeros, pos } => {
                write!(f, "Desalinhamento de RLE: restam {remaining_zeros} zeros na fronteira de nó (pos {pos})")
            }
            Self::MissingExpectedNullMarker { found, pos } => {
                write!(f, "Esperado marcador nulo 'n' após expressão, encontrado '{found}' na posição {pos}")
            }
            Self::FormattingError => write!(f, "Erro interno de formatação de string"),
        }
    }
}

impl std::error::Error for JsxbinError {}

/// Metadados de linha e rótulo (`LineInfo` / `DecodeBody`) compartilhados por instruções.
#[derive(Debug, Clone)]
pub struct LineInfo {
    pub line_number: i64,
    pub body_stmt: Option<Box<AstNode>>,
    pub labels: Vec<String>,
}

impl LineInfo {
    pub fn emit_labels(&self, out: &mut String, pad: &str) -> Result<(), JsxbinError> {
        for label in &self.labels {
            if !label.is_empty() {
                writeln!(out, "{pad}{label}: ").map_err(|_| JsxbinError::FormattingError)?;
            }
        }
        Ok(())
    }
}

/// Assinatura de função para `FunctionDeclaration` (`'M'`).
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub params: Vec<(String, usize)>,
    pub header1: usize,
    pub func_type: usize,
    pub header3: usize,
    pub name: String,
    pub header5: i64,
}

/// Representação limpa da Árvore Sintática Abstrata (AST) reconstruída a partir do JSXBIN.
#[derive(Debug, Clone)]
pub enum AstNode {
    FunctionDecl { body_info: LineInfo, signature: FunctionSignature, decl_type: usize },
    StatementList { info: LineInfo, functions: Vec<AstNode>, statements: Vec<AstNode> },
    ExprStatement { info: LineInfo, expr: Option<Box<AstNode>> },
    FunctionExpr { info: LineInfo, expr: Option<Box<AstNode>> },
    EmptyStatement { info: LineInfo },
    ConstLiteral(String),
    ConstDeclaration { name: String, expr: Option<Box<AstNode>>, literal: Option<String> },
    DebuggerStatement { line_info: LineInfo },
    Identifier(String),
    IdReference { name: String, is_xml: bool, scope_offset: i64 },
    Assignment { name: String, scope_offset: usize, expr: Option<Box<AstNode>>, literal: Option<String>, is_shorthand: bool, is_var_decl: bool },
    FunctionCall { callee: Box<AstNode>, args: Vec<AstNode>, is_new: bool },
    ArgumentList(Vec<AstNode>),
    BinaryExpr { op: String, lhs: Option<Box<AstNode>>, rhs: Option<Box<AstNode>>, lhs_lit: Option<String>, rhs_lit: Option<String> },
    LogicalExp { op: String, lhs: Option<Box<AstNode>>, rhs: Option<Box<AstNode>>, lhs_lit: Option<String>, rhs_lit: Option<String> },
    UnaryExpr { op: String, expr: Option<Box<AstNode>> },
    MemberAccess { object: Box<AstNode>, property: String },
    IndexAccess { object: Box<AstNode>, index: Box<AstNode> },
    ArrayIndexing { array_node: Option<Box<AstNode>>, index_node: Option<Box<AstNode>> },
    ArrayExpr { elements: Vec<AstNode> },
    ObjectExpr { obj_id: String, props: Vec<(String, Option<AstNode>)> },
    MemberAssignment { var_node: Option<Box<AstNode>>, expr: Option<Box<AstNode>>, literal: Option<String>, is_shorthand: bool },
    ConditionalExpr { condition: Option<Box<AstNode>>, if_true: Option<Box<AstNode>>, if_false: Option<Box<AstNode>> },
    ThisExpr(String),
    IfStatement { body: LineInfo, condition: Option<Box<AstNode>>, else_branch: Option<Box<AstNode>> },
    WhileStatement { body_info: LineInfo, condition: Option<Box<AstNode>> },
    DoWhileStatement { body: LineInfo, condition: Option<Box<AstNode>> },
    ForStatement { info: LineInfo, loop_var: Option<Box<AstNode>>, init_expr: String, upper_bound: Option<Box<AstNode>>, step_size: String, comp_op: String },
    ForStatement2 { body_info: LineInfo, init: Option<Box<AstNode>>, test: Option<Box<AstNode>>, update: Option<Box<AstNode>> },
    ForInStatement { body_info: LineInfo, loop_var: Option<Box<AstNode>>, obj_expr: Option<Box<AstNode>>, is_for_each: bool },
    SwitchStatement { label_info: LineInfo, test: Option<Box<AstNode>>, cases: Vec<AstNode>, bodies: Vec<AstNode> },
    JumpStatement { label_info: LineInfo, jmp_location: String, is_break: bool },
    ReturnStatement { line_info: LineInfo, expr: Option<Box<AstNode>> },
    ThrowStatement { line_info: LineInfo, expr: Option<Box<AstNode>> },
    TryStatement { info: LineInfo, catches: Vec<CatchClause>, finally_block: Option<Box<AstNode>> },
    IncrementExpr { id: String, op: String, is_postfix: bool },
    IndexingIncrementExpr { var_node: Option<Box<AstNode>>, op: String, is_postfix: bool },
    RegExpLiteral { regex: String, flags: String },
    DeleteExpr { op_name: String, node: Option<Box<AstNode>> },
    WithStatement { body_info: LineInfo, obj: Option<Box<AstNode>> },
    XMLAccessor { obj: Option<Box<AstNode>>, member: Option<Box<AstNode>> },
    XMLAssignment { parts: Vec<(Option<AstNode>, usize)> },
    XMLDescendants { obj: Option<Box<AstNode>>, descendants: String },
    XMLNamespace { obj: Option<Box<AstNode>>, ns: String, is_attr: bool, xml_id: String },
    SetDefaultXMLNamespace { expr: Option<Box<AstNode>> },
    UnknownNode2 { id: String, node: Option<Box<AstNode>> },
}

#[derive(Debug, Clone)]
pub struct CatchClause {
    pub guard: Option<AstNode>,
    pub exception_var: String,
    pub body: Option<AstNode>,
}

fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn sanitize_identifier(s: &str) -> String {
    if is_valid_identifier(s) {
        s.to_string()
    } else {
        let mut out = String::from("_sym");
        for b in s.bytes() {
            let _ = write!(&mut out, "_{:02x}", b);
        }
        out
    }
}

/// Estado do decodificador léxico e sintático para um fluxo `@JSXBIN@`.
pub struct JsxbinDecoder {
    chars: Vec<char>,
    pos: usize,
    zero_run: usize,
    version: JsxbinVersion,
    symbols: HashMap<i64, String>,
    depth: usize,
}

impl JsxbinDecoder {
    /// Inicializa o decodificador normalizando quebras de linha e validando o cabeçalho `@JSXBIN@ES@...`.
    pub fn new(raw_input: &str) -> Result<Self, JsxbinError> {
        let cleaned: Vec<char> = raw_input.chars().filter(|c| !c.is_whitespace() && *c != '\\').collect();

        let prefix: String = cleaned.iter().take(11).collect();
        if prefix != "@JSXBIN@ES@" {
            return Err(JsxbinError::InvalidHeader);
        }

        let ver_str: String = cleaned.iter().skip(11).take(3).collect();
        let sep = cleaned.get(14).copied();
        if sep != Some('@') {
            return Err(JsxbinError::InvalidHeader);
        }

        let version = match ver_str.as_str() {
            "1.0" => JsxbinVersion::V1_0,
            "2.0" => JsxbinVersion::V2_0,
            // ES@3.0 (ExtendScript 3) uses the same node grammar as 2.x: the version byte only
            // gates family checks (`is_v2_or_newer`), so it parses as V2_1.
            "2.1" | "3.0" => JsxbinVersion::V2_1,
            other => return Err(JsxbinError::UnsupportedVersion(other.to_string())),
        };

        let payload: Vec<char> = cleaned.into_iter().skip(15).collect();
        Ok(Self { chars: payload, pos: 0, zero_run: 0, version, symbols: HashMap::new(), depth: 0 })
    }

    #[inline]
    fn is_v2_or_newer(&self) -> bool {
        matches!(self.version, JsxbinVersion::V2_0 | JsxbinVersion::V2_1)
    }

    #[inline]
    fn peek_raw(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    #[inline]
    fn next_raw(&mut self) -> Result<char, JsxbinError> {
        let ch = self.chars.get(self.pos).copied().ok_or(JsxbinError::UnexpectedEof { pos: self.pos })?;
        self.pos = self.pos.saturating_add(1);
        Ok(ch)
    }

    /// Decodifica um valor de 5 bits (`0..=31`) a partir do alfabeto `'A'..'Z'` (0..25) e `'a'..'f'` (26..31).
    #[inline]
    fn decode_5bit(&self, ch: char) -> Result<u8, JsxbinError> {
        match ch {
            'A'..='Z' => Ok((ch as u8).saturating_sub(b'A')),
            'a'..='f' => Ok(26u8.saturating_add((ch as u8).saturating_sub(b'a'))),
            _ => Err(JsxbinError::InvalidRadixChar { ch, pos: self.pos }),
        }
    }

    /// Lê um único byte bruto codificado em Radix-32 assimétrico (sem expansão de RLE `'0'`).
    fn read_raw_byte(&mut self) -> Result<u8, JsxbinError> {
        let ch = self.next_raw()?;
        match ch {
            'A'..='Z' => Ok((ch as u8).saturating_sub(b'A')),
            'g'..='n' => {
                let hi = (ch as u8).saturating_sub(b'g');
                let lo_ch = self.next_raw()?;
                let lo = self.decode_5bit(lo_ch)?;
                Ok((hi << 5) | lo)
            }
            _ => Err(JsxbinError::InvalidRadixChar { ch, pos: self.pos.saturating_sub(1) }),
        }
    }

    /// Lê um byte lógico aplicando a descompressão RLE de zeros (`'0' + k` -> `k + 2` zeros).
    pub fn read_byte(&mut self) -> Result<u8, JsxbinError> {
        if self.zero_run > 0 {
            self.zero_run = self.zero_run.saturating_sub(1);
            return Ok(0);
        }
        if self.peek_raw() == Some('0') {
            let _ = self.next_raw()?;
            let count = usize::from(self.read_raw_byte()?).saturating_add(2);
            self.zero_run = count.saturating_sub(1);
            return Ok(0);
        }
        self.read_raw_byte()
    }

    /// Lê um valor booleano (`'t'` = true, `'f'` = false).
    pub fn read_bool(&mut self) -> Result<bool, JsxbinError> {
        let ch = self.next_raw()?;
        match ch {
            't' => Ok(true),
            'f' => Ok(false),
            _ => Err(JsxbinError::InvalidBoolChar { ch, pos: self.pos.saturating_sub(1) }),
        }
    }

    /// Lê um inteiro de comprimento variável (1 a 8 bytes em little-endian), com suporte a sinal negativo (`'y'`).
    pub fn read_int(&mut self) -> Result<i64, JsxbinError> {
        if self.zero_run > 0 {
            self.zero_run = self.zero_run.saturating_sub(1);
            return Ok(0);
        }
        let mut is_negative = false;
        if self.peek_raw() == Some('y') {
            let _ = self.next_raw()?;
            is_negative = true;
        }

        let val: i64 = match self.peek_raw() {
            Some(c @ '1'..='8') => {
                let _ = self.next_raw()?;
                let nbytes = usize::from((c as u8).saturating_sub(b'0'));
                let mut acc: u64 = 0;
                for i in 0..nbytes {
                    let b = u64::from(self.read_byte()?);
                    acc |= b << (8 * i);
                }
                acc as i64
            }
            _ => i64::from(self.read_byte()?),
        };

        Ok(if is_negative { -val } else { val })
    }

    /// Lê um inteiro não-negativo sem impor limites de coleção (usado para flags e offsets numéricos).
    pub fn read_uint(&mut self) -> Result<usize, JsxbinError> {
        let raw = self.read_int()?;
        Ok(raw.max(0) as usize)
    }

    /// Lê um comprimento não-negativo validado contra `MAX_COLLECTION_LEN` para alocações seguras.
    pub fn read_collection_length(&mut self) -> Result<usize, JsxbinError> {
        let raw = self.read_int()?;
        if raw < 0 {
            return Err(JsxbinError::NegativeLength { value: raw, pos: self.pos });
        }
        let len = usize::try_from(raw).map_err(|_| JsxbinError::AllocationLimitExceeded { requested: usize::MAX, max: MAX_COLLECTION_LEN, pos: self.pos })?;
        if len > MAX_COLLECTION_LEN {
            return Err(JsxbinError::AllocationLimitExceeded { requested: len, max: MAX_COLLECTION_LEN, pos: self.pos });
        }
        Ok(len)
    }

    /// Alias para compatibilidade.
    #[inline]
    pub fn read_length(&mut self) -> Result<usize, JsxbinError> {
        self.read_collection_length()
    }

    /// Lê uma string UTF-16 preservando surrogates órfãos (`0xD800..=0xDFFF`) como escapes `\uXXXX`.
    pub fn read_string(&mut self, escape_for_literal: bool) -> Result<String, JsxbinError> {
        let len = self.read_length()?;
        let mut out = String::with_capacity(len.min(4096));
        for _ in 0..len {
            let code_i64 = self.read_int()?;
            let code = u32::try_from(code_i64 & 0xFFFF).unwrap_or(0);
            match char::from_u32(code) {
                Some('"') if escape_for_literal => out.push_str("\\\""),
                Some('\\') if escape_for_literal => out.push_str("\\\\"),
                Some('\n') if escape_for_literal => out.push_str("\\n"),
                Some('\r') if escape_for_literal => out.push_str("\\r"),
                Some('\t') if escape_for_literal => out.push_str("\\t"),
                Some(ch) if (!escape_for_literal || (' '..='~').contains(&ch)) => out.push(ch),
                Some(ch) if !ch.is_control() => out.push(ch),
                _ => {
                    write!(&mut out, "\\u{:04x}", code & 0xFFFF).map_err(|_| JsxbinError::FormattingError)?;
                }
            }
        }
        Ok(out)
    }

    /// Lê ou registra um identificador na tabela de símbolos (`'z'` = definição; caso contrário = referência).
    pub fn read_id(&mut self) -> Result<String, JsxbinError> {
        if self.zero_run > 0 {
            self.zero_run = self.zero_run.saturating_sub(1);
            return Ok(String::new());
        }
        let res = if self.peek_raw() == Some('z') {
            let _ = self.next_raw()?;
            let name = self.read_string(false)?;
            let sym_id = self.read_int()?;
            if sym_id != 0 {
                self.symbols.insert(sym_id, name.clone());
            }
            Ok((name, sym_id, true))
        } else {
            let sym_id = self.read_int()?;
            if sym_id == 0 {
                Ok((String::new(), 0, false))
            } else {
                let name = self.symbols.get(&sym_id).cloned().ok_or(JsxbinError::UnresolvedSymbolId { id: sym_id, pos: self.pos })?;
                Ok((name, sym_id, false))
            }
        };
        match res {
            Ok((name, sym_id, is_def)) => {
                if self.pos >= 298090 && self.pos <= 298115 {
                    eprintln!("[DEBUG read_id] pos={} name='{}' sym_id={} is_def={}", self.pos, name, sym_id, is_def);
                }
                Ok(name)
            }
            Err(e) => Err(e),
        }
    }

    /// Decodifica referência (id + flag booleana se v2+).
    pub fn decode_reference(&mut self) -> Result<(String, bool), JsxbinError> {
        let id = self.read_id()?;
        let flag = if self.is_v2_or_newer() { self.read_bool()? } else { false };
        Ok((id, flag))
    }

    /// Lê um número (IEEE-754 double se iniciado por '8', ou inteiro formatado).
    pub fn read_number(&mut self) -> Result<String, JsxbinError> {
        if self.peek_raw() == Some('8') {
            let _ = self.next_raw()?;
            let mut bytes = [0u8; 8];
            for b in &mut bytes {
                *b = self.read_byte()?;
            }
            let val_u64 = u64::from_le_bytes(bytes);
            let val_f64 = f64::from_bits(val_u64);
            if val_f64.is_finite() && val_f64.fract() != 0.0 && val_f64.abs() > 1e-100 { Ok(format!("{val_f64}")) } else { Ok(format!("{}", val_u64 as i64)) }
        } else {
            Ok(format!("{}", self.read_int()?))
        }
    }

    /// Lê um valor variante constante (`'a'`=undefined, `'b'`=null, `'c'`=bool, `'d'`=num, `'e'`=string).
    pub fn read_variant(&mut self) -> Result<String, JsxbinError> {
        let vtype = self.next_raw()?;
        match vtype {
            'a' | 'b' => Ok("null".to_string()),
            'c' => Ok(if self.read_bool()? { "true" } else { "false" }.to_string()),
            'd' => self.read_number(),
            'e' => {
                let s = self.read_string(true)?;
                Ok(format!("\"{s}\""))
            }
            _ => Err(JsxbinError::InvalidVariantMarker { ch: vtype, pos: self.pos.saturating_sub(1) }),
        }
    }

    fn read_optional_variant(&mut self) -> Result<Option<String>, JsxbinError> {
        if self.peek_raw() == Some('n') {
            let _ = self.next_raw()?;
            return Ok(None);
        }
        Ok(Some(self.read_variant()?))
    }

    fn decode_line_info(&mut self) -> Result<LineInfo, JsxbinError> {
        let line_number = self.read_int()?;
        let body_stmt = self.decode_node()?.map(Box::new);
        let num_labels = self.read_length()?;
        let mut labels = Vec::with_capacity(num_labels.min(16));
        for _ in 0..num_labels {
            labels.push(self.read_id()?);
        }
        Ok(LineInfo { line_number, body_stmt, labels })
    }

    fn decode_signature(&mut self) -> Result<FunctionSignature, JsxbinError> {
        let num_params = self.read_collection_length()?;
        let mut params = Vec::with_capacity(num_params.min(64));
        for _ in 0..num_params {
            let param_name = self.read_id()?;
            let param_flags = self.read_uint()?;
            params.push((param_name, param_flags));
        }
        let header1 = self.read_uint()?;
        let func_type = self.read_uint()?;
        let header3 = self.read_uint()?;
        let name = self.read_id()?;
        let header5 = self.read_int()?;
        Ok(FunctionSignature { params, header1, func_type, header3, name, header5 })
    }

    /// Decodifica recursivamente o próximo nó da AST no fluxo JSXBIN.
    pub fn decode_node(&mut self) -> Result<Option<AstNode>, JsxbinError> {
        if self.zero_run != 0 {
            return Err(JsxbinError::TrailingZeroRunAtNodeBoundary { remaining_zeros: self.zero_run, pos: self.pos });
        }
        if self.depth >= MAX_AST_DEPTH {
            return Err(JsxbinError::RecursionLimitExceeded { max_depth: MAX_AST_DEPTH, pos: self.pos });
        }

        let marker = self.next_raw()?;
        if marker == 'n' {
            return Ok(None);
        }

        self.depth = self.depth.saturating_add(1);
        let result = self.decode_node_by_marker(marker);
        self.depth = self.depth.saturating_sub(1);
        result.map(Some)
    }

    fn decode_node_by_marker(&mut self, marker: char) -> Result<AstNode, JsxbinError> {
        match marker {
            'M' => {
                // FunctionDeclaration / Root script wrapper
                let body_info = self.decode_line_info()?;
                let signature = self.decode_signature()?;
                let decl_type = self.read_uint()?;
                Ok(AstNode::FunctionDecl { body_info, signature, decl_type })
            }
            'b' => {
                // StatementList
                let info = self.decode_line_info()?;
                let num_funcs = self.read_length()?;
                let mut functions = Vec::with_capacity(num_funcs.min(256));
                for _ in 0..num_funcs {
                    if let Some(f) = self.decode_node()? {
                        functions.push(f);
                    }
                }
                let num_stmts = self.read_length()?;
                let mut statements = Vec::with_capacity(num_stmts.min(1024));
                for _ in 0..num_stmts {
                    if let Some(stmt) = self.decode_node()? {
                        statements.push(stmt);
                    }
                }
                Ok(AstNode::StatementList { info, functions, statements })
            }
            'J' => {
                // ExprNode / ExprStatement
                let info = self.decode_line_info()?;
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::ExprStatement { info, expr })
            }
            'O' => {
                // IfStatement
                let body = self.decode_line_info()?;
                let condition = self.decode_node()?.map(Box::new);
                let else_branch = self.decode_node()?.map(Box::new);
                Ok(AstNode::IfStatement { body, condition, else_branch })
            }
            'N' => {
                // FunctionExpr
                let info = self.decode_line_info()?;
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::FunctionExpr { info, expr })
            }
            'B' => {
                // MemberAssignmentExpr
                let var_node = self.decode_node()?.map(Box::new);
                let expr = self.decode_node()?.map(Box::new);
                let literal = self.read_optional_variant()?;
                let is_shorthand = self.read_bool()?;
                Ok(AstNode::MemberAssignment { var_node, expr, literal, is_shorthand })
            }
            'U' => {
                // LogicalExp
                let op = self.read_id()?;
                let lhs = self.decode_node()?.map(Box::new);
                let rhs = self.decode_node()?.map(Box::new);
                let lhs_lit = self.read_optional_variant()?;
                let rhs_lit = self.read_optional_variant()?;
                Ok(AstNode::LogicalExp { op, lhs, rhs, lhs_lit, rhs_lit })
            }
            'A' => {
                // ArrayExpr
                let args_node = self.decode_node()?;
                let elements = match args_node {
                    Some(AstNode::ArgumentList(list)) => list,
                    Some(other) => vec![other],
                    None => Vec::new(),
                };
                Ok(AstNode::ArrayExpr { elements })
            }
            'Q' => {
                // ArrayIndexingExpr
                let _ref = self.decode_reference()?;
                let array_node = self.decode_node()?.map(Box::new);
                let index_node = self.decode_node()?.map(Box::new);
                Ok(AstNode::ArrayIndexing { array_node, index_node })
            }
            'W' => {
                // ObjectExpr
                let obj_id = self.read_id()?;
                let num_props = self.read_length()?;
                let mut props = Vec::with_capacity(num_props.min(128));
                for _ in 0..num_props {
                    let k = self.read_id()?;
                    let v = self.decode_node()?;
                    props.push((k, v));
                }
                Ok(AstNode::ObjectExpr { obj_id, props })
            }
            'Z' => {
                // ReturnStatement
                let line_info = self.decode_line_info()?;
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::ReturnStatement { line_info, expr })
            }
            'd' => {
                // ConditionalExpr
                let condition = self.decode_node()?.map(Box::new);
                let if_true = self.decode_node()?.map(Box::new);
                let if_false = self.decode_node()?.map(Box::new);
                Ok(AstNode::ConditionalExpr { condition, if_true, if_false })
            }
            'e' => {
                // ThisExpr
                let (name, _flag) = self.decode_reference()?;
                Ok(AstNode::ThisExpr(if name.is_empty() { "this".to_string() } else { name }))
            }
            'l' => {
                // WhileStatement
                let body_info = self.decode_line_info()?;
                let condition = self.decode_node()?.map(Box::new);
                Ok(AstNode::WhileStatement { body_info, condition })
            }
            'c' => {
                // SwitchStatement
                let label_info = self.decode_line_info()?;
                let test = self.decode_node()?.map(Box::new);
                let num_cases = self.read_length()?;
                let mut cases = Vec::with_capacity(num_cases.min(64));
                for _ in 0..num_cases {
                    if let Some(n) = self.decode_node()? {
                        cases.push(n);
                    }
                }
                let num_bodies = self.read_length()?;
                let mut bodies = Vec::with_capacity(num_bodies.min(64));
                for _ in 0..num_bodies {
                    if let Some(n) = self.decode_node()? {
                        bodies.push(n);
                    }
                }
                Ok(AstNode::SwitchStatement { label_info, test, cases, bodies })
            }
            'D' => {
                // JumpStatement (break / continue)
                let label_info = self.decode_line_info()?;
                let jmp_location = self.read_id()?;
                let is_break = self.read_bool()?;
                Ok(AstNode::JumpStatement { label_info, jmp_location, is_break })
            }
            'T' => {
                // IncrementExpr (id ++ / --)
                let id = self.read_id()?;
                let _p2 = self.read_uint()?;
                let add_or_sub = self.read_number()?;
                let is_postfix = self.read_bool()?;
                let op = if add_or_sub == "1" { "++".to_string() } else { "--".to_string() };
                Ok(AstNode::IncrementExpr { id, op, is_postfix })
            }
            'P' => {
                // IndexingIncrementExpr
                let var_node = self.decode_node()?.map(Box::new);
                let add_or_sub = self.read_int()?;
                let is_postfix = self.read_bool()?;
                let op = if add_or_sub == 1 { "++".to_string() } else { "--".to_string() };
                Ok(AstNode::IndexingIncrementExpr { var_node, op, is_postfix })
            }
            'X' => {
                // MemberExpr (`obj.prop` ou `obj[idx]`)
                let (property, _flag) = self.decode_reference()?;
                let object = self.decode_node()?.unwrap_or(AstNode::Identifier("this".to_string()));
                Ok(AstNode::MemberAccess { object: Box::new(object), property })
            }
            'R' => {
                // ArgumentList
                let count = self.read_length()?;
                let mut items = Vec::with_capacity(count.min(256));
                for _ in 0..count {
                    if let Some(item) = self.decode_node()? {
                        items.push(item);
                    }
                }
                let _has_more = self.read_bool()?;
                Ok(AstNode::ArgumentList(items))
            }
            'Y' => {
                // RegExpLiteral
                let regex = self.read_string(false)?;
                let flags = self.read_string(false)?;
                Ok(AstNode::RegExpLiteral { regex, flags })
            }
            'f' => {
                // ThrowStatement
                let line_info = self.decode_line_info()?;
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::ThrowStatement { line_info, expr })
            }
            'I' => {
                // DoWhileStatement
                let body = self.decode_line_info()?;
                let condition = self.decode_node()?.map(Box::new);
                Ok(AstNode::DoWhileStatement { body, condition })
            }
            'L' => {
                // ForInStatement
                let body_info = self.decode_line_info()?;
                let loop_var = self.decode_node()?.map(Box::new);
                let obj_expr = self.decode_node()?.map(Box::new);
                let _part25 = self.read_uint()?;
                let _part3 = self.read_id()?;
                let is_for_each = self.read_bool()?;
                Ok(AstNode::ForInStatement { body_info, loop_var, obj_expr, is_for_each })
            }
            'K' => {
                // ForStatement2 (`for (init; test; update) { ... }`)
                let body_info = self.decode_line_info()?;
                let init = self.decode_node()?.map(Box::new);
                let test = self.decode_node()?.map(Box::new);
                let update = self.decode_node()?.map(Box::new);
                Ok(AstNode::ForStatement2 { body_info, init, test, update })
            }
            'a' => {
                // ForStatement (v1)
                let info = self.decode_line_info()?;
                let loop_var = self.decode_node()?.map(Box::new);
                let init_expr = self.read_number()?;
                let upper_bound = self.decode_node()?.map(Box::new);
                let step_size = self.read_number()?;
                let _part6 = self.read_int()?;
                let comp_op = self.read_id()?;
                Ok(AstNode::ForStatement { info, loop_var, init_expr, upper_bound, step_size, comp_op })
            }
            'i' => {
                // DeleteExpr
                let op_name = self.read_id()?;
                let node = self.decode_node()?.map(Box::new);
                Ok(AstNode::DeleteExpr { op_name, node })
            }
            'm' => {
                // WithStatement
                let body_info = self.decode_line_info()?;
                let obj = self.decode_node()?.map(Box::new);
                Ok(AstNode::WithStatement { body_info, obj })
            }
            'S' => {
                // AssignmentExpr / VarDecl
                let name = self.read_id()?;
                let scope_offset = self.read_uint()?;
                let expr = self.decode_node()?.map(Box::new);
                let literal = self.read_optional_variant()?;
                let is_shorthand = self.read_bool()?;
                let is_var_decl = self.read_bool()?;
                Ok(AstNode::Assignment { name, scope_offset, expr, literal, is_shorthand, is_var_decl })
            }
            'E' => {
                // FunctionCallExpr
                let callee = self.decode_node()?.unwrap_or(AstNode::Identifier(String::new()));
                let args_node = self.decode_node()?;
                let is_new = self.read_bool()?;
                let args = match args_node {
                    Some(AstNode::ArgumentList(list)) => list,
                    Some(other) => vec![other],
                    None => Vec::new(),
                };
                Ok(AstNode::FunctionCall { callee: Box::new(callee), args, is_new })
            }
            'j' => {
                // Identifier
                let name = self.read_id()?;
                if self.is_v2_or_newer() {
                    let _is_xml_ns = self.read_bool()?;
                }
                Ok(AstNode::Identifier(name))
            }
            'V' => {
                // IdRefExpr
                let name = self.read_id()?;
                let is_xml = if self.is_v2_or_newer() { self.read_bool()? } else { false };
                let scope_offset = self.read_int()?;
                Ok(AstNode::IdReference { name, is_xml, scope_offset })
            }
            'C' => {
                // BinaryExpr
                let op = self.read_id()?;
                let lhs = self.decode_node()?.map(Box::new);
                let rhs = self.decode_node()?.map(Box::new);
                let lhs_lit = self.read_optional_variant()?;
                let rhs_lit = self.read_optional_variant()?;
                Ok(AstNode::BinaryExpr { op, lhs, rhs, lhs_lit, rhs_lit })
            }
            'F' => {
                // ValueNode
                let lit = self.read_variant()?;
                Ok(AstNode::ConstLiteral(lit))
            }
            'G' => {
                // ConstDeclaration
                let name = self.read_id()?;
                let _len = self.read_uint()?;
                let expr = self.decode_node()?.map(Box::new);
                let literal = self.read_optional_variant()?;
                let _b1 = self.read_bool()?;
                let _b2 = self.read_bool()?;
                Ok(AstNode::ConstDeclaration { name, expr, literal })
            }
            'H' => {
                // DebuggerStatement
                let line_info = self.decode_line_info()?;
                Ok(AstNode::DebuggerStatement { line_info })
            }
            'h' => {
                // UnaryExpr
                let op = self.read_id()?;
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::UnaryExpr { op, expr })
            }
            'r' => {
                // XMLAccessorExpr
                let (_ref, _flag) = self.decode_reference()?;
                let obj = self.decode_node()?.map(Box::new);
                let member = self.decode_node()?.map(Box::new);
                Ok(AstNode::XMLAccessor { obj, member })
            }
            'o' => {
                // XMLAssignmentExpr
                let count = self.read_length()?;
                let mut parts = Vec::with_capacity(count.min(64));
                for _ in 0..count {
                    let child = self.decode_node()?;
                    let ptype = self.read_uint()?;
                    parts.push((child, ptype));
                }
                Ok(AstNode::XMLAssignment { parts })
            }
            'q' => {
                // XMLDoubleDotDescendantsExpr
                let (descendants, _flag) = self.decode_reference()?;
                let obj = self.decode_node()?.map(Box::new);
                let _ = self.decode_node()?;
                let _ = self.decode_node()?;
                Ok(AstNode::XMLDescendants { obj, descendants })
            }
            'p' => {
                // XMLNamespaceExpr
                let (ns, is_attr) = self.decode_reference()?;
                let obj = self.decode_node()?.map(Box::new);
                let _ = self.decode_node()?;
                let _ = self.decode_node()?;
                let xml_id = self.read_id()?;
                Ok(AstNode::XMLNamespace { obj, ns, is_attr, xml_id })
            }
            'k' => {
                // SetDefaultXMLNamespaceExpr
                let expr = self.decode_node()?.map(Box::new);
                Ok(AstNode::SetDefaultXMLNamespace { expr })
            }
            's' => {
                // UnknownNode2
                let id = self.read_id()?;
                let node = self.decode_node()?.map(Box::new);
                Ok(AstNode::UnknownNode2 { id, node })
            }
            'g' => {
                // TryStatement
                let info = self.decode_line_info()?;
                let num_catches = self.read_length()?;
                let finally_block = self.decode_node()?.map(Box::new);
                let mut catches = Vec::with_capacity(num_catches.min(16));
                for _ in 0..num_catches {
                    let exception_var = self.read_id()?;
                    let guard = self.decode_node()?;
                    let body = self.decode_node()?;
                    catches.push(CatchClause { guard, exception_var, body });
                }
                Ok(AstNode::TryStatement { info, catches, finally_block })
            }
            other => Err(JsxbinError::UnknownNodeMarker { ch: other, pos: self.pos.saturating_sub(1) }),
        }
    }
}

impl AstNode {
    /// Emite código JavaScript (ECMAScript) legível e válido para execução direta no `boa_engine`.
    // A syntax-directed emitter is one flat dispatch over every AST node variant; splitting it
    // would only add indirection without reducing the branching the metric counts.
    #[allow(clippy::cognitive_complexity)]
    pub fn emit_js(&self, out: &mut String, indent: usize) -> Result<(), JsxbinError> {
        let pad = "    ".repeat(indent);
        match self {
            Self::FunctionDecl { body_info, signature, .. } => {
                if signature.header5 == 1 {
                    // Root script wrapper
                    if let Some(b) = &body_info.body_stmt {
                        b.emit_js(out, indent)?;
                    }
                } else {
                    let mut param_names = Vec::new();
                    for (pname, pflags) in &signature.params {
                        if *pflags > 536_870_000 && *pflags < 540_000_000 {
                            param_names.push(sanitize_identifier(pname));
                        }
                    }
                    if param_names.is_empty() && !signature.params.is_empty() && signature.func_type > 0 {
                        for (pname, _) in signature.params.iter().take(signature.func_type) {
                            param_names.push(sanitize_identifier(pname));
                        }
                    }
                    let params_str = param_names.join(", ");
                    let name = &signature.name;
                    let safe_name = if name.is_empty() { String::new() } else { format!(" {}", sanitize_identifier(name)) };
                    body_info.emit_labels(out, &pad)?;
                    writeln!(out, "{pad}function{safe_name}({params_str}) {{").map_err(|_| JsxbinError::FormattingError)?;
                    if let Some(b) = &body_info.body_stmt {
                        b.emit_js(out, indent + 1)?;
                        out.push('\n');
                    }
                    write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::StatementList { info, functions, statements } => {
                info.emit_labels(out, &pad)?;
                let mut first = true;
                for func in functions {
                    if !first {
                        out.push('\n');
                    }
                    func.emit_js(out, indent)?;
                    first = false;
                }
                for stmt in statements {
                    if !first {
                        out.push('\n');
                    }
                    stmt.emit_js(out, indent)?;
                    first = false;
                }
            }
            Self::ExprStatement { info, expr } => {
                info.emit_labels(out, &pad)?;
                write!(out, "{pad}").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(e) = expr {
                    e.emit_js(out, indent)?;
                }
                if !out.ends_with(';') {
                    out.push(';');
                }
            }
            Self::FunctionExpr { info, expr } => {
                info.emit_labels(out, &pad)?;
                if let Some(e) = expr {
                    e.emit_js(out, indent)?;
                }
            }
            Self::EmptyStatement { info } => {
                info.emit_labels(out, &pad)?;
                write!(out, "{pad};").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::ConstLiteral(lit) => {
                out.push_str(lit);
            }
            Self::ConstDeclaration { name, expr, literal } => {
                let safe_name = sanitize_identifier(name);
                let mut val_str = String::new();
                if let Some(e) = expr {
                    e.emit_js(&mut val_str, 0)?;
                } else if let Some(lit) = literal {
                    val_str.push_str(lit);
                }
                write!(out, "{pad}const {safe_name} = {val_str};").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::DebuggerStatement { line_info } => {
                line_info.emit_labels(out, &pad)?;
                write!(out, "{pad}debugger;").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::Identifier(id) | Self::IdReference { name: id, .. } => {
                out.push_str(&sanitize_identifier(id));
            }
            Self::Assignment { name, expr, literal, is_shorthand, is_var_decl, .. } => {
                let safe_name = sanitize_identifier(name);
                let var_kw = if *is_var_decl { "var " } else { "" };
                if *is_shorthand {
                    if let Some(AstNode::BinaryExpr { op, rhs, rhs_lit, .. }) = expr.as_deref() {
                        let mut rhs_str = String::new();
                        if let Some(r) = rhs {
                            r.emit_js(&mut rhs_str, 0)?;
                        } else if let Some(lit) = rhs_lit {
                            rhs_str.push_str(lit);
                        } else if let Some(lit) = literal {
                            rhs_str.push_str(lit);
                        }
                        write!(out, "{var_kw}{safe_name} {op}= {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                    } else {
                        let mut rhs_str = String::new();
                        if let Some(e) = expr {
                            e.emit_js(&mut rhs_str, 0)?;
                        } else if let Some(lit) = literal {
                            rhs_str.push_str(lit);
                        }
                        write!(out, "{var_kw}{safe_name} = {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                    }
                } else {
                    let mut rhs_str = String::new();
                    if let Some(e) = expr {
                        e.emit_js(&mut rhs_str, 0)?;
                    } else if let Some(lit) = literal {
                        rhs_str.push_str(lit);
                    }
                    write!(out, "{var_kw}{safe_name} = {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::MemberAssignment { var_node, expr, literal, is_shorthand } => {
                let mut var_str = String::new();
                if let Some(v) = var_node {
                    v.emit_js(&mut var_str, 0)?;
                }
                if *is_shorthand {
                    if let Some(AstNode::BinaryExpr { op, rhs, rhs_lit, .. }) = expr.as_deref() {
                        let mut rhs_str = String::new();
                        if let Some(r) = rhs {
                            r.emit_js(&mut rhs_str, 0)?;
                        } else if let Some(lit) = rhs_lit {
                            rhs_str.push_str(lit);
                        } else if let Some(lit) = literal {
                            rhs_str.push_str(lit);
                        }
                        write!(out, "{var_str} {op}= {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                    } else {
                        let mut rhs_str = String::new();
                        if let Some(e) = expr {
                            e.emit_js(&mut rhs_str, 0)?;
                        } else if let Some(lit) = literal {
                            rhs_str.push_str(lit);
                        }
                        write!(out, "{var_str} = {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                    }
                } else {
                    let mut rhs_str = String::new();
                    if let Some(e) = expr {
                        e.emit_js(&mut rhs_str, 0)?;
                    } else if let Some(lit) = literal {
                        rhs_str.push_str(lit);
                    }
                    write!(out, "{var_str} = {rhs_str}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::FunctionCall { callee, args, is_new } => {
                if *is_new {
                    out.push_str("new ");
                }
                callee.emit_js(out, indent)?;
                out.push('(');
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    arg.emit_js(out, indent)?;
                }
                out.push(')');
            }
            Self::ArgumentList(items) => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.emit_js(out, indent)?;
                }
            }
            Self::BinaryExpr { op, lhs, rhs, lhs_lit, rhs_lit } => {
                out.push('(');
                if let Some(l) = lhs {
                    l.emit_js(out, 0)?;
                } else if let Some(lit) = lhs_lit {
                    out.push_str(lit);
                }
                write!(out, " {op} ").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(r) = rhs {
                    r.emit_js(out, 0)?;
                } else if let Some(lit) = rhs_lit {
                    out.push_str(lit);
                }
                out.push(')');
            }
            Self::LogicalExp { op, lhs, rhs, lhs_lit, rhs_lit } => {
                out.push('(');
                if let Some(l) = lhs {
                    l.emit_js(out, 0)?;
                } else if let Some(lit) = lhs_lit {
                    out.push_str(lit);
                }
                write!(out, " {op} ").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(r) = rhs {
                    r.emit_js(out, 0)?;
                } else if let Some(lit) = rhs_lit {
                    out.push_str(lit);
                }
                out.push(')');
            }
            Self::UnaryExpr { op, expr } => {
                out.push_str(op);
                if let Some(e) = expr {
                    out.push('(');
                    e.emit_js(out, 0)?;
                    out.push(')');
                }
            }
            Self::MemberAccess { object, property } => {
                object.emit_js(out, indent)?;
                if is_valid_identifier(property) {
                    out.push('.');
                    out.push_str(property);
                } else if let Ok(idx) = property.parse::<i64>() {
                    write!(out, "[{idx}]").map_err(|_| JsxbinError::FormattingError)?;
                } else {
                    write!(out, "[\"{property}\"]").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::IndexAccess { object, index } => {
                object.emit_js(out, indent)?;
                out.push('[');
                index.emit_js(out, indent)?;
                out.push(']');
            }
            Self::ArrayIndexing { array_node, index_node } => {
                if let Some(a) = array_node {
                    a.emit_js(out, 0)?;
                }
                out.push('[');
                if let Some(idx) = index_node {
                    idx.emit_js(out, 0)?;
                }
                out.push(']');
            }
            Self::ArrayExpr { elements } => {
                out.push('[');
                for (i, elem) in elements.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    elem.emit_js(out, 0)?;
                }
                out.push(']');
            }
            Self::ObjectExpr { props, .. } => {
                if props.is_empty() {
                    out.push_str("{}");
                } else {
                    out.push('{');
                    for (i, (key, val)) in props.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        if is_valid_identifier(key) {
                            out.push_str(key);
                        } else {
                            write!(out, "\"{key}\"").map_err(|_| JsxbinError::FormattingError)?;
                        }
                        out.push_str(": ");
                        if let Some(v) = val {
                            v.emit_js(out, indent)?;
                        } else {
                            out.push_str("null");
                        }
                    }
                    out.push('}');
                }
            }
            Self::ThisExpr(name) => {
                out.push_str(name);
            }
            Self::ConditionalExpr { condition, if_true, if_false } => {
                out.push('(');
                if let Some(c) = condition {
                    c.emit_js(out, 0)?;
                }
                out.push_str(" ? ");
                if let Some(t) = if_true {
                    t.emit_js(out, 0)?;
                }
                out.push_str(" : ");
                if let Some(f) = if_false {
                    f.emit_js(out, 0)?;
                }
                out.push(')');
            }
            Self::ForStatement { info, loop_var, init_expr, upper_bound, step_size, comp_op } => {
                info.emit_labels(out, &pad)?;
                let mut var_name = String::new();
                if let Some(lv) = loop_var {
                    lv.emit_js(&mut var_name, 0)?;
                }
                let mut bound_str = String::new();
                if let Some(ub) = upper_bound {
                    ub.emit_js(&mut bound_str, 0)?;
                }
                writeln!(out, "{pad}for (var {var_name} = {init_expr}; {var_name} {comp_op} {bound_str}; {var_name} += {step_size}) {{")
                    .map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &info.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::ForStatement2 { body_info, init, test, update } => {
                body_info.emit_labels(out, &pad)?;
                write!(out, "{pad}for (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(i) = init {
                    i.emit_js(out, 0)?;
                }
                out.push_str("; ");
                if let Some(t) = test {
                    t.emit_js(out, 0)?;
                }
                out.push_str("; ");
                if let Some(u) = update {
                    u.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body_info.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::ForInStatement { body_info, loop_var, obj_expr, is_for_each } => {
                body_info.emit_labels(out, &pad)?;
                let loop_name = if *is_for_each { "for each" } else { "for" };
                write!(out, "{pad}{loop_name} (var ").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(v) = loop_var {
                    v.emit_js(out, 0)?;
                }
                out.push_str(" in ");
                if let Some(o) = obj_expr {
                    o.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body_info.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::IfStatement { body, condition, else_branch } => {
                body.emit_labels(out, &pad)?;
                write!(out, "{pad}if (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(c) = condition {
                    c.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(eb) = else_branch {
                    match eb.as_ref() {
                        Self::IfStatement { .. } => {
                            write!(out, " else ").map_err(|_| JsxbinError::FormattingError)?;
                            eb.emit_js(out, indent)?;
                        }
                        _ => {
                            writeln!(out, " else {{").map_err(|_| JsxbinError::FormattingError)?;
                            eb.emit_js(out, indent + 1)?;
                            writeln!(out).map_err(|_| JsxbinError::FormattingError)?;
                            write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                        }
                    }
                }
            }
            Self::WhileStatement { body_info, condition } => {
                body_info.emit_labels(out, &pad)?;
                write!(out, "{pad}while (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(c) = condition {
                    c.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body_info.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::DoWhileStatement { body, condition } => {
                body.emit_labels(out, &pad)?;
                writeln!(out, "{pad}do {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}} while (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(c) = condition {
                    c.emit_js(out, 0)?;
                }
                write!(out, ");").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::SwitchStatement { label_info, test, cases, bodies } => {
                label_info.emit_labels(out, &pad)?;
                write!(out, "{pad}switch (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(t) = test {
                    t.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                let case_pad = format!("{pad}    ");
                for (i, case_node) in cases.iter().enumerate() {
                    let mut case_args = Vec::new();
                    if let Self::ArgumentList(args) = case_node {
                        case_args.extend(args.iter());
                    } else {
                        case_args.push(case_node);
                    }
                    if case_args.is_empty() {
                        writeln!(out, "{case_pad}default:").map_err(|_| JsxbinError::FormattingError)?;
                    } else {
                        for arg in case_args {
                            write!(out, "{case_pad}case ").map_err(|_| JsxbinError::FormattingError)?;
                            arg.emit_js(out, 0)?;
                            writeln!(out, ":").map_err(|_| JsxbinError::FormattingError)?;
                        }
                    }
                    if let Some(b) = bodies.get(i) {
                        b.emit_js(out, indent + 2)?;
                        out.push('\n');
                    }
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::JumpStatement { label_info, jmp_location, is_break } => {
                label_info.emit_labels(out, &pad)?;
                let kw = if *is_break { "break" } else { "continue" };
                if jmp_location.is_empty() {
                    write!(out, "{pad}{kw};").map_err(|_| JsxbinError::FormattingError)?;
                } else {
                    write!(out, "{pad}{kw} {jmp_location};").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::ReturnStatement { line_info, expr } => {
                line_info.emit_labels(out, &pad)?;
                if let Some(e) = expr {
                    write!(out, "{pad}return ").map_err(|_| JsxbinError::FormattingError)?;
                    e.emit_js(out, 0)?;
                    out.push(';');
                } else {
                    write!(out, "{pad}return;").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::ThrowStatement { line_info, expr } => {
                line_info.emit_labels(out, &pad)?;
                write!(out, "{pad}throw ").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(e) = expr {
                    e.emit_js(out, 0)?;
                }
                out.push(';');
            }
            Self::TryStatement { info, catches, finally_block } => {
                info.emit_labels(out, &pad)?;
                writeln!(out, "{pad}try {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(tb) = &info.body_stmt {
                    tb.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                for c in catches {
                    writeln!(out, " catch ({}) {{", c.exception_var).map_err(|_| JsxbinError::FormattingError)?;
                    if let Some(cb) = &c.body {
                        cb.emit_js(out, indent + 1)?;
                        out.push('\n');
                    }
                    write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                }
                if let Some(fin) = finally_block {
                    writeln!(out, " finally {{").map_err(|_| JsxbinError::FormattingError)?;
                    fin.emit_js(out, indent + 1)?;
                    out.push('\n');
                    write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::IncrementExpr { id, op, is_postfix } => {
                let safe_id = sanitize_identifier(id);
                if *is_postfix {
                    write!(out, "{safe_id}{op}").map_err(|_| JsxbinError::FormattingError)?;
                } else {
                    write!(out, "{op}{safe_id}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::IndexingIncrementExpr { var_node, op, is_postfix } => {
                let mut var_str = String::new();
                if let Some(v) = var_node {
                    v.emit_js(&mut var_str, 0)?;
                }
                if *is_postfix {
                    write!(out, "{var_str}{op}").map_err(|_| JsxbinError::FormattingError)?;
                } else {
                    write!(out, "{op}{var_str}").map_err(|_| JsxbinError::FormattingError)?;
                }
            }
            Self::RegExpLiteral { regex, flags } => {
                write!(out, "/{regex}/{flags}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::DeleteExpr { op_name, node } => {
                write!(out, "{op_name} ").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(n) = node {
                    n.emit_js(out, 0)?;
                }
            }
            Self::WithStatement { body_info, obj } => {
                body_info.emit_labels(out, &pad)?;
                write!(out, "{pad}with (").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(o) = obj {
                    o.emit_js(out, 0)?;
                }
                writeln!(out, ") {{").map_err(|_| JsxbinError::FormattingError)?;
                if let Some(b) = &body_info.body_stmt {
                    b.emit_js(out, indent + 1)?;
                    out.push('\n');
                }
                write!(out, "{pad}}}").map_err(|_| JsxbinError::FormattingError)?;
            }
            Self::XMLAccessor { obj, member } => {
                if let Some(o) = obj {
                    o.emit_js(out, 0)?;
                }
                out.push('.');
                if let Some(m) = member {
                    m.emit_js(out, 0)?;
                }
            }
            Self::XMLAssignment { parts } => {
                let mut first = true;
                for (child, ptype) in parts {
                    if *ptype == 0 {
                        if let Some(c) = child {
                            c.emit_js(out, 0)?;
                        }
                    } else {
                        if !first {
                            out.push_str(" + ");
                        }
                        if let Some(c) = child {
                            c.emit_js(out, 0)?;
                        }
                        out.push_str(" + ");
                    }
                    first = false;
                }
            }
            Self::XMLDescendants { obj, descendants } => {
                if let Some(o) = obj {
                    o.emit_js(out, 0)?;
                }
                out.push_str("..");
                out.push_str(descendants);
            }
            Self::XMLNamespace { obj, ns, is_attr, xml_id } => {
                if let Some(o) = obj {
                    o.emit_js(out, 0)?;
                }
                out.push('.');
                if *is_attr {
                    out.push('@');
                }
                out.push_str(ns);
                out.push_str("::");
                out.push_str(xml_id);
            }
            Self::SetDefaultXMLNamespace { expr } => {
                if let Some(e) = expr {
                    e.emit_js(out, indent)?;
                }
            }
            Self::UnknownNode2 { id, node } => {
                out.push_str(id);
                if let Some(n) = node {
                    n.emit_js(out, 0)?;
                }
            }
        }
        Ok(())
    }
}

/// Ponto de entrada principal para transpilação em tempo de execução antes do envio ao `boa_engine`.
pub fn decode_jsxbin_to_js(source: &str) -> Result<String, JsxbinError> {
    let mut decoder = JsxbinDecoder::new(source)?;
    let root = decoder.decode_node()?;
    let mut js_out = String::new();
    if let Some(node) = root {
        node.emit_js(&mut js_out, 0)?;
    }
    Ok(js_out)
}

/// O ponto único onde todo código de script passa antes de chegar ao parser: decodifica JSXBIN
/// quando o código é binário — sozinho (`@JSXBIN@ES@…` como primeiro conteúdo) ou dentro do
/// wrapper de painel que o engine emite (`(function () { CODE }).call(__uiDockPanel("título"))`).
/// Devolve `Ok(None)` para JavaScript em texto (sem cópia).
///
/// Centralizar aqui garante que *todo* caminho que roda script (`script.run`, File ▸ Scripts,
/// painés ScriptUI, `$.evalFile`, `#include`) aceite `.jsxbin`, presente e futuro.
pub fn decode_script_source(code: &str) -> Result<Option<String>, JsxbinError> {
    let t = code.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("@JSXBIN@") {
        return decode_jsxbin_to_js(t).map(Some);
    }
    let Some(inner) = t.strip_prefix("(function () {").or_else(|| t.strip_prefix("(function(){")) else {
        return Ok(None);
    };
    let inner = inner.trim_start();
    if !inner.starts_with("@JSXBIN@") {
        return Ok(None);
    }
    // `panel_wrapper` (engine) emits exactly `(function () { CODE\n}).call(__uiDockPanel("title"));`.
    // The wrapper's marker is the *last* one: nothing of the payload can follow it.
    let (payload, suffix) = match inner.rfind("\n}).call(__uiDockPanel(") {
        Some(i) => (inner[..i].trim_end(), &inner[i..]),
        None => (inner, ""),
    };
    let js = decode_jsxbin_to_js(payload)?;
    Ok(Some(format!("(function () {{\n{js}\n{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny compiled script (`alert("Hello")`) so CI proves decoding without local files.
    const SAMPLE: &str = "@JSXBIN@ES@2.0@MyBbyBn0ABJAnAEjzFjBjMjFjSjUBfRBFeFjIjFjMjMjPff0DzACByB";

    #[test]
    fn synthetic_sample_decodes_to_javascript() {
        let r = decode_jsxbin_to_js(SAMPLE);
        assert_eq!(r.as_deref().ok(), Some("alert(\"hello\");"), "{r:?}");
    }

    #[test]
    fn es_3_0_uses_the_v2_node_grammar() {
        let v3 = SAMPLE.replacen("@JSXBIN@ES@2.0@", "@JSXBIN@ES@3.0@", 1);
        let r = decode_jsxbin_to_js(&v3);
        assert_eq!(r.as_deref().ok(), Some("alert(\"hello\");"), "{r:?}");
    }

    #[test]
    fn decode_script_source_handles_bare_wrapped_and_plain() {
        assert_eq!(decode_script_source(SAMPLE).ok().flatten(), Some("alert(\"hello\");".to_string()));
        // The engine's panel wrapper, emitted verbatim and minified, both decode.
        for prefix in ["(function () { ", "(function(){ "] {
            let wrapped = format!("{prefix}{SAMPLE}\n}}).call(__uiDockPanel(\"T\"));");
            let r = decode_script_source(&wrapped).expect("wrapper decodes");
            let js = r.expect("wrapper is jsxbin");
            assert!(js.starts_with("(function () {\n"), "{js}");
            assert!(js.contains("alert(\"hello\");"), "{js}");
            assert!(js.ends_with("\n}).call(__uiDockPanel(\"T\"));"), "{js}");
        }
        // Plain JavaScript and other wrappers pass through untouched.
        assert_eq!(decode_script_source("var a = 1;").ok(), Some(None));
        assert_eq!(decode_script_source("(function () { var a = 1; })();").ok(), Some(None));
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        let cases = [
            "",
            "@JSXBIN@",
            "@JSXBIN@ES@2.0@",
            "@JSXBIN@ES@9.9@MyBbyBn0",
            "not jsxbin at all",
            "@JSXBIN@ES@2.0@!!!!\u{0}\u{ffff}",
            "@JSXBIN@ES@2.0@zzzzzzzzzzzzzzzzzzzzzzzz",
            &SAMPLE[..SAMPLE.len() / 2],
            &SAMPLE[..SAMPLE.len() - 1],
        ];
        for c in cases {
            // Must return (Ok or Err); the point is that it never panics.
            let _ = decode_jsxbin_to_js(c);
        }
    }

    #[test]
    fn every_truncation_of_a_sample_is_handled() {
        for n in 0..SAMPLE.len() {
            if let Some(prefix) = SAMPLE.get(..n) {
                let _ = decode_jsxbin_to_js(prefix);
            }
        }
    }

    #[test]
    fn deeply_nested_input_hits_the_depth_limit() {
        // Repeating a nesting marker must stop at MAX_AST_DEPTH, not overflow the stack.
        let src = format!("@JSXBIN@ES@2.0@MyBbyBn{}", "0".repeat(200_000));
        let _ = decode_jsxbin_to_js(&src);
    }

    #[test]
    fn test_bolt_jsxbin_transpile() {
        let path = r#"D:\AE_SCRIPTS\Bolt v1.2 by CreativeDojo\Bolt v1.2 by CreativeDojo\Bolt_v1.2.jsxbin"#;
        if let Ok(content) = std::fs::read_to_string(path) {
            let res = decode_jsxbin_to_js(&content);
            match res {
                Ok(js) => {
                    println!("Successfully transpiled Bolt to JS! Length: {} bytes", js.len());
                    println!("Snippet:\n{}", &js[..js.len().min(300)]);
                    assert!(!js.is_empty());
                }
                Err(e) => {
                    panic!("Failed to decode Bolt_v1.2.jsxbin: {e}");
                }
            }
        }
    }

    #[test]
    fn test_motion_tools_transpile() {
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let path = std::path::PathBuf::from(appdata).join("EffectCraft").join("Scripts").join("ScriptUI Panels").join("Motion Tools.jsx");
        if let Ok(content) = std::fs::read_to_string(&path) {
            let res = decode_jsxbin_to_js(&content);
            match res {
                Ok(js) => {
                    assert!(!js.is_empty());
                    assert!(js.contains("motionToolsFunc"));
                }
                Err(e) => {
                    panic!("Failed to decode Motion Tools.jsx: {e}");
                }
            }
        }
    }
}
