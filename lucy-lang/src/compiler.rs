#![allow(unused)]

use std::collections::{HashMap, HashSet};

use crate::ty::{Type, FunctionType, TypeArena, TypeId, ClassType};

use crate::vm::{
    Opcode, RuntimeValue, ConstantValue, FunctionProto,
    UpvalueDescriptor, UpvalueSource, NativeFunctionProto,
    pack_abx, pack_abc,
};

use crate::parser::{
    AstNode, SAst, BindingNode, TypeNode, ClassMember, MatchArm, MatchPattern,
    MacroArm, MacroTokenTree, MacroDesignator, MacroDefinition, LucyParser, RepQuantifier
};

use crate::lexer::Token;
use crate::operator::Operator;
use crate::span::Span;

use crate::builtin_types::{BuiltinClass};

pub fn span_to_location(source: &str, span: Span) -> String {
    if span == Span::dummy() {
        return "<unknown location>".to_string();
    }
    let up_to = &source[..span.start.min(source.len())];
    let line   = up_to.bytes().filter(|&b| b == b'\n').count() + 1;
    let col    = up_to.rfind('\n').map(|p| span.start - p - 1).unwrap_or(span.start) + 1;
    format!("{}:{}", line, col)
}

fn compile_error_at(source: &str, span: Span, msg: &str) -> ! {
    panic!("CompileError at {}: {}", span_to_location(source, span), msg)
}

#[derive(Clone)]
pub struct VariadicTemplate {
    pub fixed_params: Vec<BindingNode>,
    pub vararg_type:  TypeNode,
    pub return_type:  TypeNode,
    pub body:         Box<SAst>,
}

#[derive(Clone)]
pub struct PendingSpecialization {
    pub name:         String,
    pub vararg_count: usize,
    pub ctx:          CompilingCtx,
}

#[derive(Debug)]
pub struct Local {
    pub reg:     usize,
    pub ty:      Type,
    pub backing: Option<ConstantValue>,
    pub mutable: bool,
    pub moved:   bool,
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub children:        HashMap<String, Namespace>,
    pub locals:          HashMap<String, (usize, bool)>,
    pub types:           HashMap<String, Type>,
    pub constants:       HashMap<String, ConstantValue>,
    pub exported_macros: HashMap<String, MacroDefinition>,
}

impl Namespace {
    pub fn new() -> Self {
        Self {
            children:        HashMap::new(),
            locals:          HashMap::new(),
            types:           HashMap::new(),
            constants:       HashMap::new(),
            exported_macros: HashMap::new(),
        }
    }
}
pub struct NamespaceBuilder<'a> {
    compiler:  &'a mut LucyCompiler,
    namespace: Namespace,
}

impl<'a> NamespaceBuilder<'a> {
    pub fn new(compiler: &'a mut LucyCompiler) -> Self {
        Self { compiler, namespace: Namespace::new() }
    }

    pub fn construct(
        compiler: &mut LucyCompiler,
        build: impl FnOnce(NamespaceBuilder) -> NamespaceBuilder,
    ) -> Namespace
    {
        build(NamespaceBuilder::new(compiler)).build()
    }

    pub fn function(
        mut self,
        name:  &str,
        arity: u8,
        func:  fn(Vec<RuntimeValue>) -> RuntimeValue,
    ) -> Self {
        let idx = self.compiler.lulib_register_native_fn(name, arity, func);
        self.namespace.locals.insert(name.to_string(), (idx, true));
        self
    }

    pub fn build(self) -> Namespace { self.namespace }
}

#[derive(Debug)]
pub struct Scope {
    pub locals:          HashMap<String, Local>,
    pub exports:         HashMap<String, usize>,
    pub types:           HashMap<String, Type>,
    pub exported_types:  std::collections::HashSet<String>,
    pub namespaces:      HashMap<String, Namespace>,
    pub exported_macros: HashMap<String, MacroDefinition>,
    pub proto_depth:     usize,
    reg_base:            usize,
}

pub struct RegisterAllocator {
    pub current_top: usize,
}

impl RegisterAllocator {
    fn alloc(&mut self) -> usize {
        let r = self.current_top;
        self.current_top += 1;
        r
    }
    fn free_to(&mut self, top: usize) {
        self.current_top = top;
    }
}

#[derive(Debug)]
pub enum LocalResolution {
    Local      { reg: usize, ty: Type, backing: Option<ConstantValue>, mutable: bool, moved: bool },
    OuterProto { reg: usize, ty: Type, backing: Option<ConstantValue>, mutable: bool, moved: bool },
}

#[derive(Debug)]
pub struct ScopeStack {
    pub scopes: Vec<Scope>,
}

impl ScopeStack {
    fn new() -> Self { Self { scopes: vec![] } }

    fn push(&mut self, reg_base: usize, proto_depth: usize) {
        self.scopes.push(Scope {
            locals:          HashMap::new(),
            exports:         HashMap::new(),
            types:           HashMap::new(),
            exported_types:  std::collections::HashSet::new(),
            namespaces:      HashMap::new(),
            exported_macros: HashMap::new(),
            proto_depth,
            reg_base,
        });
    }

    fn pop(&mut self) -> usize {
        self.scopes.pop().expect("popped empty scope stack").reg_base
    }

    pub fn mark_moved(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.locals.get_mut(name) {
                local.moved = true;
                return;
            }
        }
    }

    pub fn define_local(
        &mut self,
        name: String, reg: usize, ty: Type,
        backing: Option<ConstantValue>, mutable: bool,
    ) {
        self.get_current_scope_mut().locals.insert(
            name, Local { reg, ty, backing, mutable, moved: false },
        );
    }

    pub fn define_export(
        &mut self,
        name: String, reg: usize, ty: Type,
        backing: Option<ConstantValue>, mutable: bool,
    ) {
        let scope = self.get_current_scope_mut();
        scope.exports.insert(name.clone(), reg);
        scope.locals.insert(name, Local { reg, ty, backing, mutable, moved: false });
    }

    pub fn define_type(&mut self, name: String, ty: Type) {
        self.get_current_scope_mut().types.insert(name, ty);
    }

    pub fn export_type(&mut self, name: String, ty: Type) {
        let scope = self.get_current_scope_mut();
        scope.types.insert(name.clone(), ty);
        scope.exported_types.insert(name);
    }

    pub fn lookup_type(&self, name: &str) -> Option<&Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.types.get(name) { return Some(t); }
        }
        None
    }

    pub fn resolve_local(&self, name: &str, current_proto_depth: usize) -> Option<LocalResolution> {
        for scope in self.scopes.iter().rev() {
            if let Some(local) = scope.locals.get(name) {
                let res = if scope.proto_depth == current_proto_depth {
                    LocalResolution::Local {
                        reg:     local.reg,
                        ty:      local.ty.clone(),
                        backing: local.backing.clone(),
                        mutable: local.mutable,
                        moved:   local.moved,
                    }
                } else {
                    LocalResolution::OuterProto {
                        reg:     local.reg,
                        ty:      local.ty.clone(),
                        backing: local.backing.clone(),
                        mutable: local.mutable,
                        moved:   local.moved,
                    }
                };
                return Some(res);
            }
        }
        None
    }

    pub fn resolve_namespace_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut Namespace> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(ns) = scope.namespaces.get_mut(name) {
                return Some(ns);
            }
        }
        None
    }

    pub fn define_namespace(&mut self, name: String, ns: Namespace) {
        self.get_current_scope_mut().namespaces.insert(name, ns);
    }

    pub fn get_current_scope(&self) -> &Scope {
        self.scopes.last().expect("no active scope")
    }
    pub fn get_current_scope_mut(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("no active scope")
    }
}

#[derive(Clone)]
pub struct CompilingCtx {
    pub is_public:     bool,
    pub current_class: Option<String>,
    pub is_inside_vararg: bool,
}

impl CompilingCtx {
    fn new() -> Self {
        Self { is_public: false, current_class: None, is_inside_vararg: false }
    }
}

pub struct LucyCompiler {
    pub reg_alloc:          RegisterAllocator,
    pub scopes:             ScopeStack,
    pub proto_stack:        Vec<FunctionProto>,
    pub native_protos:      Vec<NativeFunctionProto>,
    pub native_namespaces:  HashMap<String, Namespace>,
    pub proto_depth:        usize,
    pub type_arena:         TypeArena,
    pub macros:             HashMap<String, MacroDefinition>,
    pub module_registry: HashMap<String, Namespace>,
    pub root_proto_idx:     usize,
    pub variadic_templates:         HashMap<String, VariadicTemplate>,
    pub pending_specializations:    Vec<PendingSpecialization>,
    pub completed_specializations:  HashSet<(String, usize)>,
    pub current_vararg_regs:        Vec<usize>,
    current_namespace_name: Option<String>,
    source:                 String,
    current_span:           Span,
}

impl LucyCompiler {
    pub fn lulib_register_native_fn(
        &mut self, name: &str, arity: u8, func: fn(Vec<RuntimeValue>) -> RuntimeValue,
    ) -> usize {
        let idx = self.native_protos.len();
        self.native_protos.push(NativeFunctionProto {
            name: name.to_string(), arity, func,
        });
        idx
    }

    pub fn lulib_register_namespace(
        &mut self,
        name:  &str,
        build: impl FnOnce(NamespaceBuilder) -> NamespaceBuilder,
    ) {
        let ns = build(NamespaceBuilder::new(self)).build();
        self.scopes.get_current_scope_mut().namespaces.insert(name.to_string(), ns);
    }
}

impl LucyCompiler {
    pub fn new(source: String) -> Self {
        let mut s = Self {
            reg_alloc:          RegisterAllocator { current_top: 0 },
            scopes:             ScopeStack::new(),
            proto_stack:        vec![],
            native_protos:      vec![],
            native_namespaces:  HashMap::new(),
            proto_depth:        0,
            type_arena:         TypeArena::new(),
            source,
            current_span:       Span::dummy(),
            macros:             HashMap::new(),
            module_registry: HashMap::new(),
            root_proto_idx:     0,
            current_namespace_name: None,
            completed_specializations: HashSet::new(),
            current_vararg_regs: vec![],
            pending_specializations: vec![],
            variadic_templates: HashMap::new()
        };
        s.enter_proto("__main__".to_string(), 0);
        s.enter_scope();
        s
    }

    fn compile_error(&self, msg: &str) -> ! {
        compile_error_at(&self.source, self.current_span, msg)
    }

    #[inline]
    fn set_span(&mut self, span: Span) -> Span {
        let prev = self.current_span;
        self.current_span = span;
        prev
    }

    pub fn compile(&mut self, program: &SAst) {
        let ctx = CompilingCtx::new();
        self.compile_stmt(program, &ctx);
        self.emit(pack_abc(Opcode::RET as u32, 0, 0, 0));
    }

    pub fn enter_scope(&mut self) {
        let base = self.reg_alloc.current_top;
        self.scopes.push(base, self.proto_depth);
    }

    pub fn exit_scope(&mut self) {
        let base = self.scopes.pop();
        self.reg_alloc.free_to(base);
    }

    pub fn current_proto(&mut self) -> &mut FunctionProto {
        self.proto_stack.last_mut().expect("no active proto")
    }

    pub fn current_proto_immut(&self) -> &FunctionProto {
        self.proto_stack.last().expect("no active proto")
    }

    fn emit(&mut self, op: u32) -> usize {
        let proto = self.current_proto();
        proto.code.push(op);
        proto.code.len() - 1
    }

    fn add_constant(&mut self, c: ConstantValue) -> usize {
        if let Some(i) = self.current_proto().constants.iter().position(|x| *x == c) {
            return i;
        }
        let proto = self.current_proto();
        proto.constants.push(c);
        proto.constants.len() - 1
    }

    pub fn root_proto(&self) -> &FunctionProto {
        &self.proto_stack[self.root_proto_idx]
    }

    fn enter_proto(&mut self, name: String, arity: u8) {
        self.proto_depth += 1;
        let saved_top = self.reg_alloc.current_top;
        self.reg_alloc.current_top = 0;
        self.proto_stack.push(FunctionProto {
            name, arity, max_regs: 0,
            code: vec![], constants: vec![], protos: vec![],
            upvalues: vec![], saved_reg_top: saved_top,
        });
    }

    fn exit_proto(&mut self) -> FunctionProto {
        self.proto_depth -= 1;
        let proto = self.proto_stack.pop().expect("no active proto");
        self.reg_alloc.current_top = proto.saved_reg_top;
        proto
    }

    fn capture_upvalue(&mut self, name: &str, source: UpvalueSource, ty: Type) -> usize {
        if let Some(idx) = self.current_proto().upvalues.iter().position(|u| u.name == name) {
            return idx;
        }
        let uv  = UpvalueDescriptor { name: name.to_string(), source, ty };
        let idx = self.current_proto().upvalues.len();
        self.current_proto().upvalues.push(uv);
        idx
    }
}

impl LucyCompiler {
    pub fn register_builtin_class(&mut self, class: &BuiltinClass) {
        use std::collections::HashMap;
        use crate::ty::{ClassType, FunctionType};
        use crate::vm::{ConstantValue, Opcode, pack_abx};

        let mut fields          = Vec::new();
        let mut field_index_map = HashMap::new();

        for (i, f) in class.fields.iter().enumerate() {
            field_index_map.insert(f.name.clone(), i);
            fields.push((f.name.clone(), f.ty.clone(), f.is_public));
        }

        let mut methods     = HashMap::new();
        let field_vis: Vec<bool> = fields.iter().map(|(_, _, p)| *p).collect();

        // No real method protos for builtins (no body) — just register signatures
        for (i, m) in class.methods.iter().enumerate() {
            let fn_ty = FunctionType {
                params:      m.params.iter().map(|(_, t)| t.clone()).collect(),
                return_type: Box::new(m.return_type.clone()),
            };
            methods.insert(m.name.clone(), (i, 0usize, fn_ty, m.is_public));
        }

        let class_id = self.type_arena.alloc_class(ClassType {
            name:                 class.name.clone(),
            fields,
            field_index_map,
            methods,
            operators:            HashMap::new(),
            class_proto_constant: None,
        });

        // Build the ClassProto constant so NEWCLASS works at runtime
        let class_proto = ConstantValue::ClassProto {
            name:      class.name.clone(),
            fields:    field_vis,
            methods:   vec![],   // no method protos for pure-data builtins
            operators: HashMap::new(),
        };

        self.type_arena.get_class_mut(class_id).class_proto_constant = Some(class_proto);
        self.scopes.define_type(class.name.clone(), Type::Class(class_id));
    }
}

impl LucyCompiler {
    fn emit_jmp(&mut self, offset: i32) {
        let a = (offset + 128) as u32;
        self.emit(pack_abc(Opcode::JMP as u32, a, 0, 0));
    }

    fn patch_jmp(&mut self, idx: usize, target_pc: usize) {
        let jump_pc = idx + 1;
        let offset  = target_pc as i32 - jump_pc as i32;
        let a       = (offset + 128) as u32;
        let instr   = &mut self.current_proto().code[idx];
        *instr      = (*instr & !(0xFF << 6)) | ((a & 0xFF) << 6);
    }
}

impl LucyCompiler {
    fn compile_type(&self, node: &TypeNode) -> Type {
        match node {
            TypeNode::Inferred => Type::Unknown,
            TypeNode::ArrayType { elem_type } =>
                Type::Array(Box::new(self.compile_type(elem_type))),
            TypeNode::Qualified { inner, mutable, borrowed, moved } =>
                Type::Qualified {
                    inner:    Box::new(self.compile_type(inner)),
                    mutable:  *mutable,
                    borrowed: *borrowed,
                    moved:    *moved,
                },
            TypeNode::NominalType { name, generics } => {
                let args: Vec<Type> = generics.iter().map(|g| self.compile_type(g)).collect();
                if let Some(ty) = Self::resolve_builtin(name) { return ty; }
                if let Some(class_ty) = self.scopes.lookup_type(name) {
                    return class_ty.clone();
                }
                if args.is_empty() { Type::TypeVar(name.clone()) }
                else               { Type::Generic { name: name.clone(), args } }
            }
            other => panic!("Unhandled type {:?}", other)
        }
    }

    fn resolve_builtin(name: &str) -> Option<Type> {
        match name {
            "u8"      => Some(Type::U8),    "i8"      => Some(Type::I8),
            "u16"     => Some(Type::U16),   "i16"     => Some(Type::I16),
            "u32"     => Some(Type::U32),   "i32"     => Some(Type::I32),
            "u64"     => Some(Type::U64),   "i64"     => Some(Type::I64),
            "usize"   => Some(Type::USize), "bool"    => Some(Type::Bool),
            "string"  => Some(Type::String),"empty"   => Some(Type::Empty),
            _         => None,
        }
    }

    /// Infer the type of an expression — used only for codegen decisions
    /// (field index lookup, operator overload dispatch, upvalue capture type).
    /// No type errors are emitted here.
    fn infer_expr_type(&self, expr: &AstNode, ctx: &CompilingCtx) -> Type {
        match expr {
            AstNode::IntLiteral(_)    => Type::I32,
            AstNode::FloatLiteral(_)  => Type::F64,
            AstNode::StringLiteral(_) => Type::String,
            AstNode::Typeof(_)        => Type::TypeVar("Type".to_string()),
            AstNode::SelfExpr => {
                ctx.current_class.as_ref()
                    .and_then(|n| self.scopes.lookup_type(n))
                    .cloned()
                    .unwrap_or(Type::Unknown)
            }
            AstNode::Identifier(name) => {
                match self.scopes.resolve_local(name, self.proto_depth) {
                    Some(LocalResolution::Local { ty, .. })
                    | Some(LocalResolution::OuterProto { ty, .. }) => ty,
                    None => Type::Unknown,
                }
            }
            AstNode::Borrowed(inner) => {
                let inner_ty = self.infer_expr_type(&inner.node, ctx);
                Type::Qualified { inner: Box::new(inner_ty), mutable: false, borrowed: true, moved: false }
            }
            AstNode::Moved(inner) => {
                let inner_ty = self.infer_expr_type(&inner.node, ctx);
                Type::Qualified { inner: Box::new(inner_ty), mutable: false, borrowed: false, moved: true }
            }
            AstNode::BinaryOperation { op, left, right } => {
                let lt = self.infer_expr_type(&left.node, ctx);
                let rt = self.infer_expr_type(&right.node, ctx);
                if matches!(lt, Type::F64) || matches!(rt, Type::F64) { return Type::F64; }
                if matches!(lt, Type::F32) || matches!(rt, Type::F32) { return Type::F32; }
                if lt != Type::Unknown { lt } else { rt }
            }
            AstNode::FunctionCall { callee, .. } => {
                let callee_ty = self.infer_expr_type(&callee.node, ctx);
                match callee_ty {
                    Type::Function(ft) => *ft.return_type,
                    _ => Type::Unknown,
                }
            }
            AstNode::DotIndex { indexee, index } => {
                // First: try to resolve as a static namespace member
                let mut parts = Vec::new();
                Self::collect_dot_chain(&indexee.node, &mut parts);
                let member_name = match &index.node {
                    AstNode::Identifier(s) => s.as_str(),
                    _ => return Type::Unknown,
                };
                let ns_owner = parts.last().cloned().unwrap_or_default();
                let qualified = format!("{}::{}", ns_owner, member_name);
                if let Some(res) = self.scopes.resolve_local(&qualified, self.proto_depth) {
                    match res {
                        LocalResolution::Local { ty, .. } | LocalResolution::OuterProto { ty, .. } => return ty,
                    }
                }
                // Then: try instance field/method
                let obj_ty = self.infer_expr_type(&indexee.node, ctx);
                if let Type::Class(id) = &obj_ty {
                    let class = self.type_arena.get_class(*id);
                    if let Some((_, ty, _)) = class.fields.iter().find(|(n, _, _)| n == member_name) {
                        return ty.clone();
                    }
                    if let Some((_, _, fn_ty, _)) = class.methods.get(member_name) {
                        return Type::Function(Box::new(fn_ty.clone()));
                    }
                }
                Type::Unknown
            }
            AstNode::ClassLiteral { ty, .. } => {
                let class_name = match &ty.node {
                    AstNode::Identifier(s) => s.clone(),
                    AstNode::SelfExpr => ctx.current_class.clone().unwrap_or_default(),
                    _ => return Type::Unknown,
                };
                self.scopes.lookup_type(&class_name).cloned().unwrap_or(Type::Unknown)
            }
            _ => Type::Unknown,
        }
    }
}

impl LucyCompiler {
    fn compile_block_into(&mut self, node: &SAst, dst: usize, ctx: &CompilingCtx) {
        match &node.node {
            AstNode::Block(stmts) => {
                self.enter_scope();
                if stmts.is_empty() {
                    // Empty block = Empty value, nothing to emit
                } else {
                    let (body, tail) = stmts.split_at(stmts.len() - 1);
                    for s in body { self.compile_stmt(s, ctx); }
                    // Tail expression goes into dst
                    self.compile_expr(&tail[0], dst, ctx);
                }
                self.exit_scope();
            }
            _ => {
                // Single expression body
                self.compile_expr(node, dst, ctx);
            }
        }
    }

    fn compile_if(
        &mut self,
        condition: &SAst,
        body:      &SAst,
        else_body: Option<&SAst>,
        ctx:       &CompilingCtx,
    ) {
        let saved    = self.reg_alloc.current_top;
        let cond_reg = self.reg_alloc.alloc();
        self.compile_expr(condition, cond_reg, ctx);

        let false_reg = self.reg_alloc.alloc();
        let k = self.add_constant(ConstantValue::I32(0));
        self.emit(pack_abx(Opcode::LOADK as u32, false_reg as u32, k as u32));

        let jump_over_body = self.current_proto().code.len();
        self.emit(pack_abc(Opcode::JEQ as u32, 128, cond_reg as u32, false_reg as u32));

        self.reg_alloc.free_to(saved);

        match &body.node {
            AstNode::Block(stmts) => {
                self.enter_scope();
                for s in stmts { self.compile_stmt(s, ctx); }
                self.exit_scope();
            }
            _ => { self.compile_stmt(body, ctx); }
        }

        if let Some(else_node) = else_body {
            let jump_over_else = self.current_proto().code.len();
            self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));
            let else_start = self.current_proto().code.len();
            self.patch_jmp(jump_over_body, else_start);

            match &else_node.node {
                AstNode::Block(stmts) => {
                    self.enter_scope();
                    for s in stmts { self.compile_stmt(s, ctx); }
                    self.exit_scope();
                }
                AstNode::ConditionalBranch { condition, body, else_body } => {
                    self.compile_if(condition, body, else_body.as_deref(), ctx);
                }
                _ => { self.compile_stmt(else_node, ctx); }
            }

            let end_pc = self.current_proto().code.len();
            self.patch_jmp(jump_over_else, end_pc);
        } else {
            let end_pc = self.current_proto().code.len();
            self.patch_jmp(jump_over_body, end_pc);
        }
    }

    fn compile_while(&mut self, condition: &SAst, body: &SAst, ctx: &CompilingCtx) {
        let saved      = self.reg_alloc.current_top;
        let cond_start = self.current_proto().code.len();
        let cond_reg   = self.reg_alloc.alloc();

        self.compile_expr(condition, cond_reg, ctx);

        let false_reg = cond_reg + 1;
        if self.reg_alloc.current_top <= false_reg {
            self.reg_alloc.current_top = false_reg + 1;
        }
        let k = self.add_constant(ConstantValue::Bool(false));
        self.emit(pack_abx(Opcode::LOADK as u32, false_reg as u32, k as u32));

        let exit_jump = self.current_proto().code.len();
        self.emit(pack_abc(Opcode::JEQ as u32, 128, cond_reg as u32, false_reg as u32));

        self.reg_alloc.free_to(saved);
        self.enter_scope();
        match &body.node {
            AstNode::Block(stmts) => {
                for stmt in stmts { self.compile_stmt(stmt, ctx); }
            }
            _ => { self.compile_stmt(body, ctx); }
        }
        self.exit_scope();

        self.reg_alloc.free_to(saved);

        let current_pc  = self.current_proto().code.len();
        let back_offset = cond_start as i32 - current_pc as i32;
        self.emit_jmp(back_offset);

        let end_pc = self.current_proto().code.len();
        self.patch_jmp(exit_jump, end_pc);
    }

    fn compile_for(
        &mut self, binding: &BindingNode, iterator: &SAst, body: &SAst, ctx: &CompilingCtx,
    ) {
        if ctx.is_inside_vararg
            && matches!(iterator.node, AstNode::Ellipsis)
        {
            let bind_name = match binding {
                BindingNode::IdentifierBinding { name, .. } => name.clone(),
                _ => "__arg".to_string(),
            };

            for &vreg in &self.current_vararg_regs.clone() {
                self.enter_scope();

                self.scopes.define_local(
                    bind_name.clone(),
                    vreg,
                    Type::Unknown,
                    None,
                    false,
                );

                self.compile_stmt(body, ctx);

                self.exit_scope();
            }

            return;
        }
        self.enter_scope();
        let saved       = self.reg_alloc.current_top;
        let iter_reg    = self.reg_alloc.alloc();
        self.compile_expr(iterator, iter_reg, ctx);

        let current_reg = self.reg_alloc.alloc();
        let end_reg     = self.reg_alloc.alloc();
        self.emit(pack_abc(Opcode::GETFIELD as u32, current_reg as u32, iter_reg as u32, 0));
        self.emit(pack_abc(Opcode::GETFIELD as u32, end_reg as u32,     iter_reg as u32, 1));

        if let BindingNode::IdentifierBinding { name, ty } = binding {
            let compiled_ty = self.compile_type(ty);
            self.scopes.define_local(name.clone(), current_reg, compiled_ty, None, false);
        }

        let loop_start = self.current_proto().code.len();
        let ge_reg     = self.reg_alloc.alloc();
        self.emit(pack_abc(Opcode::GE as u32, ge_reg as u32, current_reg as u32, end_reg as u32));

        let true_reg = self.reg_alloc.alloc();
        let k        = self.add_constant(ConstantValue::I32(1));
        self.emit(pack_abx(Opcode::LOADK as u32, true_reg as u32, k as u32));

        let exit_jump = self.current_proto().code.len();
        self.emit(pack_abc(Opcode::JEQ as u32, 128, ge_reg as u32, true_reg as u32));

        self.reg_alloc.free_to(saved + 3);
        match &body.node {
            AstNode::Block(stmts) => {
                for stmt in stmts { self.compile_stmt(stmt, ctx); }
            }
            _ => { self.compile_stmt(body, ctx); }
        }

        let one_reg = self.reg_alloc.alloc();
        let k       = self.add_constant(ConstantValue::I32(1));
        self.emit(pack_abx(Opcode::LOADK as u32, one_reg as u32, k as u32));
        self.emit(pack_abc(Opcode::ADD as u32, current_reg as u32, current_reg as u32, one_reg as u32));
        self.reg_alloc.free_to(saved + 3);

        let loop_end    = self.current_proto().code.len() + 1;
        let back_offset = loop_start as i32 - loop_end as i32;
        self.emit_jmp(back_offset);

        let end_pc = self.current_proto().code.len();
        self.patch_jmp(exit_jump, end_pc);
        self.exit_scope();
    }

    fn compile_match(&mut self, matchee: &SAst, arms: &[MatchArm], ctx: &CompilingCtx) {
        let saved       = self.reg_alloc.current_top;
        let matchee_reg = self.reg_alloc.alloc();
        self.compile_expr(matchee, matchee_reg, ctx);

        let mut exit_jumps: Vec<usize> = Vec::new();

        for arm in arms {
            match &arm.pattern {
                MatchPattern::Expr(pat_expr) => {
                    let pat_reg = self.reg_alloc.alloc();
                    self.compile_expr(pat_expr, pat_reg, ctx);

                    let skip_jump = self.current_proto().code.len();
                    self.emit(pack_abc(Opcode::JNE as u32, 128, matchee_reg as u32, pat_reg as u32));
                    self.reg_alloc.free_to(saved + 1);

                    self.enter_scope();
                    match &arm.body.node {
                        AstNode::Block(stmts) => {
                            for stmt in stmts { self.compile_stmt(stmt, ctx); }
                        }
                        _ => { self.compile_stmt(&arm.body, ctx); }
                    }
                    self.exit_scope();

                    let exit_jump = self.current_proto().code.len();
                    self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));
                    exit_jumps.push(exit_jump);

                    let next_arm = self.current_proto().code.len();
                    self.patch_jmp(skip_jump, next_arm);
                }
                MatchPattern::Binding(BindingNode::IdentifierBinding { name, ty }) => {
                    self.enter_scope();
                    let bind_reg    = self.reg_alloc.alloc();
                    self.emit(pack_abc(Opcode::MOVE as u32, bind_reg as u32, matchee_reg as u32, 0));
                    let compiled_ty = self.compile_type(ty);
                    self.scopes.define_local(name.clone(), bind_reg, compiled_ty, None, false);

                    match &arm.body.node {
                        AstNode::Block(stmts) => {
                            for stmt in stmts { self.compile_stmt(stmt, ctx); }
                        }
                        _ => { self.compile_stmt(&arm.body, ctx); }
                    }
                    self.exit_scope();

                    let exit_jump = self.current_proto().code.len();
                    self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));
                    exit_jumps.push(exit_jump);
                }
                other => panic!("Unhandled match pattern: {:?}", other),
            }
        }

        let end_pc = self.current_proto().code.len();
        for jump_idx in exit_jumps { self.patch_jmp(jump_idx, end_pc); }
        self.reg_alloc.free_to(saved);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statement compilation
// ─────────────────────────────────────────────────────────────────────────────

impl LucyCompiler {
    fn compile_stmt(&mut self, stmt: &SAst, ctx: &CompilingCtx) {
        self.set_span(stmt.span);

        match &stmt.node {
            AstNode::Program(stmts) => {
                self.enter_scope();
                for node in stmts { self.compile_stmt(node, ctx); }
                
                self.drain_specializations();
                self.exit_scope();
            }

            AstNode::ModuleStmt { name } => {
                self.current_namespace_name = Some(name.clone());
            }

            AstNode::GlobalDeclaration { name, init_value } => {
                let src = self.reg_alloc.alloc();
                self.compile_expr(init_value, src, ctx);

                let name_k = self.add_constant(ConstantValue::String(name.clone()));
                self.emit(pack_abx(Opcode::SETGLOBAL as u32, src as u32, name_k as u32));

                self.scopes.define_local(name.clone(), src, 
                    self.infer_expr_type(&init_value.node, ctx), None, false);
            }

            AstNode::UseStmt { base_path, used } => {
                let mut path_parts = Vec::<String>::new();
                Self::collect_dot_chain(&base_path.node, &mut path_parts);

                // Auto-inject root into scopes from registry if missing
                if let Some(root) = path_parts.first().cloned() {
                    if Self::find_namespace_in_scopes(&self.scopes, &root).is_none() {
                        if let Some(ns) = self.module_registry.get(&root).cloned() {
                            self.scopes.define_namespace(root, ns);
                        }
                    }
                }

                if used.is_empty() {
                    let leaf = path_parts.last().cloned().unwrap_or_default();
                    // Pull from module registry first, then fall back to scopes
                    let ns = self.module_registry.get(&leaf).cloned()
                        .or_else(|| Self::find_namespace_chained_static(&self.scopes, &path_parts).cloned())
                        .unwrap_or_else(|| panic!("unknown module '{}'", leaf));
                    self.scopes.define_namespace(leaf, ns);
                    return;
                }

                // For deconstructed imports, look in scopes (already-used modules) then registry
                let namespace = Self::find_namespace_chained_static(&self.scopes, &path_parts)
                    .cloned()
                    .or_else(|| {
                        if path_parts.len() == 1 {
                            self.module_registry.get(&path_parts[0]).cloned()
                        } else { None }
                    })
                    .unwrap_or_else(|| panic!("unknown namespace '{}'", path_parts.join(".")));

                let resolved: Vec<(String, ConstantValue)> = used
                    .iter()
                    .filter_map(|(actual, alias)| {
                        namespace.constants.get(actual)
                            .map(|cv| (alias.clone(), cv.clone()))
                            .or_else(|| {
                                namespace.locals.get(actual).map(|(idx, _)| {
                                    (alias.clone(), ConstantValue::NativeFunctionProto(*idx))
                                })
                            })
                    })
                    .collect();

                let type_imports: Vec<(String, Type, Option<Namespace>)> = used
                    .iter()
                    .filter_map(|(actual, alias)| {
                        namespace.types.get(actual).map(|ty| {
                            let child_ns = namespace.children.get(actual).cloned();
                            (alias.clone(), ty.clone(), child_ns)
                        })
                    })
                    .collect();

                let macro_imports: Vec<(String, MacroDefinition)> = used
                    .iter()
                    .filter_map(|(actual, alias)| {
                        namespace.exported_macros.get(actual).map(|mac| {
                            let mut m = mac.clone();
                            m.name = alias.clone();
                            (alias.clone(), m)
                        })
                    })
                    .collect();

                for (alias, cv) in resolved {
                    let const_idx = self.add_constant(cv.clone());
                    let dst       = self.reg_alloc.alloc();
                    self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, const_idx as u32));
                    self.scopes.define_local(alias, dst, Type::Unknown, Some(cv), false);
                }

                for (alias, ty, child_ns) in type_imports {
                    self.scopes.define_type(alias.clone(), ty.clone());

                    let method_info: Vec<(String, usize, FunctionType, bool)> =
                        if let Type::Class(id) = &ty {
                            let class = self.type_arena.get_class(*id);
                            class.methods.iter().map(|(mname, (_, proto_idx, fn_ty, is_pub))| {
                                (mname.clone(), *proto_idx, fn_ty.clone(), *is_pub)
                            }).collect()
                        } else {
                            vec![]
                        };

                    for (method_name, proto_idx, fn_ty, _) in method_info {
                        let cv       = ConstantValue::FunctionProto(proto_idx);
                        let full_name = format!("{}::{}", alias, method_name);
                        let const_idx = self.add_constant(cv.clone());
                        let dst       = self.reg_alloc.alloc();
                        self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, const_idx as u32));
                        self.scopes.define_local(
                            full_name, dst,
                            Type::Function(Box::new(fn_ty)),
                            Some(cv), false,
                        );
                    }

                    if let Some(ns) = child_ns {
                        self.scopes.define_namespace(alias.clone(), ns);
                    }
                }

                for (alias, mac) in macro_imports {
                    self.macros.insert(alias, mac);
                }
            }

            AstNode::Public(inner) => {
                let mut pub_ctx = ctx.clone();
                pub_ctx.is_public = true;
                self.compile_stmt(inner, &pub_ctx);

                if let AstNode::MacroDefinitionNode(def) = &inner.node {
                    let mac = def.clone();
                    self.scopes.get_current_scope_mut()
                        .exported_macros.insert(def.name.clone(), mac);
                }

                if let AstNode::ClassDefinition { name, .. } = &inner.node {
                    let name = name.clone();
                    self.scopes.get_current_scope_mut()
                        .exported_types.insert(name);
                }
            }

            AstNode::VarDeclaration { binding, init_value } => {
                match binding {
                    BindingNode::IdentifierBinding { name, ty } => {
                        let dst         = self.reg_alloc.alloc();
                        let declared_ty = self.compile_type(ty);
                        let is_mutable  = declared_ty.is_mutable();

                        let resolved_ty = if matches!(declared_ty, Type::Unknown) {
                            if let Some(expr) = init_value {
                                self.infer_expr_type(&expr.node, ctx)
                            } else { Type::Unknown }
                        } else { declared_ty.clone() };

                        if ctx.is_public {
                            self.scopes.define_export(name.clone(), dst, resolved_ty.clone(), None, is_mutable);
                        } else {
                            self.scopes.define_local(name.clone(), dst, resolved_ty.clone(), None, is_mutable);
                        }

                        if let Some(expr) = init_value {
                            self.compile_expr(expr, dst, ctx);
                        }
                    }
                    other => panic!("Unhandled binding in VarDeclaration: {:?}", other),
                }
            }

            AstNode::Block(stmts) => {
                self.enter_scope();
                for s in stmts { self.compile_stmt(s, ctx); }
                self.exit_scope();
            }

            AstNode::FunctionDeclaration { name, params, vararg, return_type, body, .. } => {
                if vararg.is_some() {
                    self.variadic_templates.insert(name.clone(), VariadicTemplate {
                        fixed_params: params.clone(),
                        vararg_type:  vararg.clone().unwrap(),
                        return_type:  return_type.clone(),
                        body:         body.clone(),
                    });
                    let dst = self.reg_alloc.alloc();
                    self.scopes.define_local(
                        name.clone(), dst,
                        Type::Unknown, None, false,
                    );
                } else {
                    self.compile_function_decl(name, params, return_type, body, false, ctx);
                }
            }

            AstNode::ConditionalBranch { condition, body, else_body } => {
                self.compile_if(condition, body, else_body.as_deref(), ctx);
            }

            AstNode::WhileLoop { condition, body } => {
                self.compile_while(condition, body, ctx);
            }

            AstNode::ForLoop { binding, iterator, body } => {
                self.compile_for(binding, iterator, body, ctx);
            }

            AstNode::MatchStmt { matchee, arms } => {
                self.compile_match(matchee, arms, ctx);
            }

            AstNode::ClassDefinition { name, members } => {
                self.compile_class_definition(name, members, ctx);
            }

            AstNode::ReturnStmt { value } => {
                match value {
                    Some(expr) => {
                        let src = self.reg_alloc.alloc();
                        self.compile_expr(expr, src, ctx);
                        self.emit(pack_abc(Opcode::RET as u32, src as u32, 1, 0));
                        self.reg_alloc.free_to(src);
                    }
                    None => {
                        self.emit(pack_abc(Opcode::RET as u32, 0, 0, 0));
                    }
                }
            }

            AstNode::FunctionCall { .. } | AstNode::BinaryOperation { .. } => {
                let scratch = self.reg_alloc.alloc();
                self.compile_expr(stmt, scratch, ctx);
                self.reg_alloc.free_to(scratch);
            }

            AstNode::Assignment { left, right } => {
                match &left.node {
                    AstNode::Identifier(name) => {
                        match self.scopes.resolve_local(name, self.proto_depth) {
                            Some(LocalResolution::Local { reg, mutable, .. }) => {
                                if !mutable {
                                    self.compile_error(&format!(
                                        "cannot assign to immutable variable '{}'", name
                                    ));
                                }
                                self.compile_expr(right, reg, ctx);
                            }
                            Some(LocalResolution::OuterProto { .. }) =>
                                self.compile_error("cannot assign to variable captured from outer scope"),
                            None =>
                                self.compile_error(&format!("undefined variable '{}'", name)),
                        }
                    }
                    AstNode::DotIndex { indexee, index } => {
                        let saved_top = self.reg_alloc.current_top;
                        let obj_reg = match &indexee.node {
                            AstNode::SelfExpr => {
                                match self.scopes.resolve_local("self", self.proto_depth).unwrap() {
                                    LocalResolution::Local { reg, .. }
                                    | LocalResolution::OuterProto { reg, .. } => reg,
                                }
                            }
                            _ => {
                                let r = self.reg_alloc.alloc();
                                self.compile_expr(indexee, r, ctx);
                                r
                            }
                        };
                        let field_name = match &index.node {
                            AstNode::Identifier(s) => s.clone(),
                            other => self.compile_error(&format!(
                                "DotIndex assignment: expected ident, got {:?}", other
                            )),
                        };
                        let obj_ty = self.infer_expr_type(&indexee.node, ctx);
                        if self.reg_alloc.current_top <= obj_reg {
                            self.reg_alloc.current_top = obj_reg + 1;
                        }
                        let val_reg = self.reg_alloc.alloc();
                        self.compile_expr(right, val_reg, ctx);
                        let field_index = match &obj_ty {
                            Type::Class(id) => {
                                let class = self.type_arena.get_class(*id);
                                *class.field_index_map.get(&field_name).unwrap_or_else(|| {
                                    self.compile_error(&format!("unknown field '{}'", field_name))
                                })
                            }
                            _ => self.compile_error("DotIndex assignment on non-class value"),
                        };
                        self.emit(pack_abc(
                            Opcode::SETFIELD as u32,
                            obj_reg as u32, val_reg as u32, field_index as u32,
                        ));
                        self.reg_alloc.free_to(saved_top);
                    }
                    other => self.compile_error(&format!(
                        "unhandled assignment left-hand side: {:?}", other
                    )),
                }
            }

            AstNode::MacroDefinitionNode(def) => {
                self.macros.insert(def.name.clone(), def.clone());
            }

            AstNode::MacroInvocation { name, args } => {
                let expanded = self.expand_macro_to_ast(name, args, stmt.span);
                for node in expanded { self.compile_stmt(&node, ctx); }
            }

            other => self.compile_error(&format!("Unhandled statement node: {:?}", other)),
        }
    }
}

impl LucyCompiler {
    pub fn drain_specializations(&mut self) {
        while let Some(pending) = self.pending_specializations.pop() {
            let key = (pending.name.clone(), pending.vararg_count);

            if self.completed_specializations.contains(&key) {
                continue;
            }

            self.completed_specializations.insert(key);

            let template = self
                .variadic_templates
                .get(&pending.name)
                .cloned()
                .unwrap();

            self.compile_variadic_specialization(
                &pending.name,
                pending.vararg_count,
                &template,
                &pending.ctx,
            );
        }
    }

    fn compile_variadic_specialization(
        &mut self,
        name: &str,
        vararg_count: usize,
        template: &VariadicTemplate,
        ctx: &CompilingCtx,
    ) {
        let mangled = format!("{}#{}", name, vararg_count);

        let mut all_params = template.fixed_params.clone();
        let vararg_regs_start = all_params.len();

        for i in 0..vararg_count {
            all_params.push(BindingNode::IdentifierBinding {
                name: format!("__v{}", i),
                ty: template.vararg_type.clone(),
            });
        }

        let arity = all_params.len() as u8;

        self.enter_proto(mangled.clone(), arity);
        self.enter_scope();

        let mut vararg_regs = Vec::new();

        for (i, param) in all_params.iter().enumerate() {
            if let BindingNode::IdentifierBinding {
                name: pname,
                ty,
            } = param
            {
                let compiled_ty = self.compile_type(ty);

                self.scopes.define_local(
                    pname.clone(),
                    i,
                    compiled_ty,
                    None,
                    false,
                );

                self.reg_alloc.alloc();

                if i >= vararg_regs_start {
                    vararg_regs.push(i);
                }
            }
        }

        let saved_vararg_regs =
            std::mem::replace(&mut self.current_vararg_regs, vararg_regs);

        let mut fn_ctx = ctx.clone();
        fn_ctx.is_public = false;
        fn_ctx.is_inside_vararg = true;

        self.compile_stmt(&template.body, &fn_ctx);

        self.current_vararg_regs = saved_vararg_regs;

        self.exit_scope();

        let proto = self.exit_proto();

        let proto_idx = {
            let parent = self.current_proto();
            let idx = parent.protos.len();
            parent.protos.push(proto);
            idx
        };

        let cv_key = ConstantValue::String(mangled.clone());

        for c in self.current_proto().constants.iter_mut() {
            if *c == cv_key {
                *c = ConstantValue::FunctionProto(proto_idx);
            }
        }

        let cv = ConstantValue::FunctionProto(proto_idx);

        let const_idx = self.add_constant(cv.clone());

        let dst = self.reg_alloc.alloc();

        self.emit(pack_abx(
            Opcode::LOADK as u32,
            dst as u32,
            const_idx as u32,
        ));

        self.scopes.define_local(
            mangled,
            dst,
            Type::Function(Box::new(FunctionType {
                params: all_params
                    .iter()
                    .map(|p| match p {
                        BindingNode::IdentifierBinding { ty, .. } => {
                            self.compile_type(ty)
                        }
                        _ => Type::Unknown,
                    })
                    .collect(),
                return_type: Box::new(
                    self.compile_type(&template.return_type),
                ),
            })),
            Some(cv),
            false,
        );
    }
}

impl LucyCompiler {
    fn compile_class_definition(
        &mut self,
        class_name: &str,
        members: &[ClassMember],
        ctx: &CompilingCtx,
    ) {
        let class_id = self.type_arena.alloc_class(ClassType {
            name: class_name.to_string(),
            fields: vec![],
            field_index_map: HashMap::new(),
            methods: HashMap::new(),
            operators: HashMap::new(),
            class_proto_constant: None,
        });

        if ctx.is_public {
            self.scopes.export_type(class_name.to_string(), Type::Class(class_id));
        } else {
            self.scopes.define_type(class_name.to_string(), Type::Class(class_id));
        }

        let mut field_types     = Vec::new();
        let mut field_index_map = HashMap::new();

        for m in members {
            if let ClassMember::Field { name, ty, is_public } = m {
                let t = self.compile_type(ty);
                let resolved_ty = match t {
                    Type::TypeVar(ref n) if n == "Self" || n.as_str() == class_name => {
                        self.scopes.lookup_type(class_name).cloned().unwrap_or(Type::Unknown)
                    }
                    other => other,
                };
                field_index_map.insert(name.clone(), field_types.len());
                field_types.push((name.clone(), resolved_ty, *is_public));
            }
        }

        {
            let class = self.type_arena.get_class_mut(class_id);
            class.fields         = field_types;
            class.field_index_map = field_index_map;
        }

        let mut class_ctx = ctx.clone();
        class_ctx.current_class = Some(class_name.to_string());

        let mut ns                  = Namespace::new();
        let mut method_proto_indices: HashMap<String, usize> = HashMap::new();
        let mut op_proto_indices:     HashMap<String, usize> = HashMap::new();

        for m in members {
            if let ClassMember::Method { name: method_name, params, has_self, is_public, .. } = m {
                let arity = params.len() as u8 + if *has_self { 1 } else { 0 };

                let placeholder = FunctionProto {
                    name: format!("{}::{}", class_name, method_name),
                    arity,
                    max_regs: 0,
                    code: vec![], constants: vec![], protos: vec![], upvalues: vec![],
                    saved_reg_top: self.reg_alloc.current_top,
                };

                let local_idx = {
                    let parent = self.current_proto();
                    let idx    = parent.protos.len();
                    parent.protos.push(placeholder);
                    idx
                };

                method_proto_indices.insert(method_name.clone(), local_idx);

                let class      = self.type_arena.get_class_mut(class_id);
                let method_idx = class.methods.len();
                class.methods.insert(
                    method_name.clone(),
                    (method_idx, local_idx, FunctionType { params: vec![], return_type: Box::new(Type::Unknown) }, *is_public),
                );

                ns.locals.insert(method_name.clone(), (local_idx, *is_public));
                ns.constants.insert(method_name.clone(), ConstantValue::FunctionProto(local_idx));
            } else if let ClassMember::OperatorOverload { op, .. } = m {
                let op_name = format!("{}::operator@{:?}", class_name, op);

                let placeholder = FunctionProto {
                    name: op_name.clone(),
                    arity: 2,
                    max_regs: 0,
                    code: vec![], constants: vec![], protos: vec![], upvalues: vec![],
                    saved_reg_top: self.reg_alloc.current_top,
                };

                let local_idx = {
                    let parent = self.current_proto();
                    let idx    = parent.protos.len();
                    parent.protos.push(placeholder);
                    idx
                };

                op_proto_indices.insert(op_name.clone(), local_idx);

                let class = self.type_arena.get_class_mut(class_id);
                class.operators.insert(
                    op.clone(),
                    (local_idx, FunctionType { params: vec![], return_type: Box::new(Type::Unknown) }),
                );
            }
        }

        {
            let class    = self.type_arena.get_class(class_id);
            let field_vis: Vec<bool> = class.fields.iter().map(|(_, _, p)| *p).collect();

            let mut ordered = vec![(404usize, false); class.methods.len()];
            for (_, (method_idx, proto_idx, _, is_public)) in &class.methods {
                ordered[*method_idx] = (*proto_idx, *is_public);
            }

            let mut operators = HashMap::new();
            for (op, (proto_idx, _)) in &class.operators {
                operators.insert(op.clone(), *proto_idx);
            }

            self.type_arena.get_class_mut(class_id).class_proto_constant = Some(
                ConstantValue::ClassProto {
                    name:      class_name.to_string(),
                    fields:    field_vis,
                    methods:   ordered,
                    operators,
                }
            );
        }

        self.scopes.define_namespace(class_name.to_string(), ns);

        for m in members {
            if let ClassMember::Method {
                name: method_name, has_self, params, return_type, body, ..
            } = m {
                let mut all_params = Vec::new();
                if *has_self {
                    all_params.push(BindingNode::IdentifierBinding {
                        name: "self".to_string(),
                        ty:   TypeNode::NominalType { name: class_name.to_string(), generics: vec![] },
                    });
                }
                all_params.extend_from_slice(params);

                let full_name = format!("{}::{}", class_name, method_name);
                let local_idx = method_proto_indices[method_name];

                self.enter_proto(full_name.clone(), all_params.len() as u8);
                self.enter_scope();

                for (i, param) in all_params.iter().enumerate() {
                    if let BindingNode::IdentifierBinding { name: pname, ty } = param {
                        let compiled_ty = self.compile_type(ty);
                        self.scopes.define_local(pname.clone(), i, compiled_ty, None, false);
                        self.reg_alloc.alloc();
                    }
                }

                let mut fn_ctx = class_ctx.clone();
                fn_ctx.is_public = false;

                match &body.node {
                    AstNode::Block(stmts) => {
                        for stmt in stmts { self.compile_stmt(stmt, &fn_ctx); }
                    }
                    _ => { self.compile_stmt(body, &fn_ctx); }
                }

                let needs_implicit_ret = match &body.node {
                    AstNode::Block(stmts) => {
                        for stmt in stmts { self.compile_stmt(stmt, &fn_ctx); }
                        !matches!(stmts.last().map(|n| &n.node), Some(AstNode::ReturnStmt { .. }))
                    }
                    AstNode::ReturnStmt { .. } => {
                        self.compile_stmt(body, &fn_ctx);
                        false
                    }
                    _ => {
                        self.compile_stmt(body, &fn_ctx);
                        true
                    }
                };

                if needs_implicit_ret {
                    self.emit(pack_abc(Opcode::RET as u32, 0, 0, 0));
                }

                self.exit_scope();
                let real_proto = self.exit_proto();
                self.current_proto().protos[local_idx] = real_proto;

                let fn_ty = FunctionType {
                    params: all_params.iter().map(|p| match p {
                        BindingNode::IdentifierBinding { ty, .. } => self.compile_type(ty),
                        _ => Type::Unknown,
                    }).collect(),
                    return_type: Box::new(self.compile_type(return_type)),
                };

                let cv        = ConstantValue::FunctionProto(local_idx);
                let const_idx = self.add_constant(cv.clone());
                let reg       = self.reg_alloc.alloc();
                self.emit(pack_abx(Opcode::LOADK as u32, reg as u32, const_idx as u32));
                self.scopes.define_local(
                    full_name.clone(), reg,
                    Type::Function(Box::new(fn_ty.clone())),
                    Some(cv.clone()), false,
                );

                {
                    let ns = self.scopes.resolve_namespace_mut(class_name).unwrap();
                    ns.constants.insert(method_name.clone(), cv.clone());
                    ns.locals.insert(method_name.clone(), (local_idx, true));
                }

                self.type_arena.get_class_mut(class_id)
                    .methods.get_mut(method_name).unwrap().2 = fn_ty;

            } else if let ClassMember::OperatorOverload { op, params, return_type, body } = m {
                let full_name = format!("{}::operator@{:?}", class_name, op);
                let local_idx = op_proto_indices[&full_name];

                self.enter_proto(full_name.clone(), params.len() as u8);
                self.enter_scope();

                for (i, param) in params.iter().enumerate() {
                    if let BindingNode::IdentifierBinding { name: pname, ty } = param {
                        let compiled_ty = self.compile_type(ty);
                        self.scopes.define_local(pname.clone(), i, compiled_ty, None, false);
                        self.reg_alloc.alloc();
                    }
                }

                let mut fn_ctx = class_ctx.clone();
                fn_ctx.is_public = false;
                
                match &body.node {
                    AstNode::Block(stmts) => {
                        for stmt in stmts { self.compile_stmt(stmt, &fn_ctx); }
                    }
                    _ => { self.compile_stmt(body, &fn_ctx); }
                }

                self.exit_scope();
                let real_proto = self.exit_proto();
                self.current_proto().protos[local_idx] = real_proto;

                let fn_ty = FunctionType {
                    params: params.iter().map(|p| match p {
                        BindingNode::IdentifierBinding { ty, .. } => self.compile_type(ty),
                        _ => Type::Unknown,
                    }).collect(),
                    return_type: Box::new(self.compile_type(return_type)),
                };

                let cv = ConstantValue::FunctionProto(local_idx);
                self.scopes.define_local(
                    full_name, 0,
                    Type::Function(Box::new(fn_ty.clone())),
                    Some(cv), false,
                );

                self.type_arena.get_class_mut(class_id)
                    .operators.get_mut(op).unwrap().1 = fn_ty;
            }
        }
    }

    fn compile_function_decl(
        &mut self,
        name:        &str,
        params:      &[BindingNode],
        return_type: &TypeNode,
        body:        &SAst,   // <-- was &[SAst]
        is_method:   bool,
        ctx:         &CompilingCtx,
    ) -> (usize, usize, FunctionType) {
        let arity = params.len() as u8;
        self.enter_proto(name.to_string(), arity);
        self.enter_scope();

        for (i, param) in params.iter().enumerate() {
            match param {
                BindingNode::IdentifierBinding { name: pname, ty } => {
                    let compiled_ty = self.compile_type(ty);
                    self.scopes.define_local(pname.clone(), i, compiled_ty, None, false);
                    self.reg_alloc.alloc();
                }
                other => self.compile_error(&format!("Unhandled param binding: {:?}", other)),
            }
        }

        let declared_ret = self.compile_type(return_type);
        let mut fn_ctx   = ctx.clone();
        fn_ctx.is_public = false;

        // Infer return type from tail if not declared
        let inferred_ret = match &body.node {
            AstNode::Block(stmts) => stmts.last()
                .map(|t| self.infer_expr_type(&t.node, &fn_ctx))
                .unwrap_or(Type::Empty),
            _ => self.infer_expr_type(&body.node, &fn_ctx),
        };

        // Compile body — tail expression becomes implicit return
        let ret_reg = self.reg_alloc.alloc();
        match &body.node {
            AstNode::Block(stmts) => {
                if !stmts.is_empty() {
                    let (head, tail) = stmts.split_at(stmts.len() - 1);
                    for s in head { self.compile_stmt(s, &fn_ctx); }
                    let tail_node = &tail[0];
                    // If tail is already a return, compile as stmt; else compile into ret_reg
                    match &tail_node.node {
                        AstNode::ReturnStmt { .. } => {
                            self.compile_stmt(tail_node, &fn_ctx);
                        }
                        _ => {
                            self.compile_expr(tail_node, ret_reg, &fn_ctx);
                            self.emit(pack_abc(Opcode::RET as u32, ret_reg as u32, 1, 0));
                        }
                    }
                } else {
                    self.emit(pack_abc(Opcode::RET as u32, 0, 0, 0));
                }
            }
            _ => {
                match &body.node {
                    AstNode::ReturnStmt { .. } => {
                        self.compile_stmt(body, &fn_ctx);
                    }
                    _ => {
                        self.compile_expr(body, ret_reg, &fn_ctx);
                        self.emit(pack_abc(Opcode::RET as u32, ret_reg as u32, 1, 0));
                    }
                }
            }
        }

        self.exit_scope();
        let proto = self.exit_proto();

        let proto_local_idx = {
            let parent = self.current_proto();
            let idx    = parent.protos.len();
            parent.protos.push(proto);
            idx
        };

        let final_ret = if matches!(declared_ret, Type::Unknown) { inferred_ret } else { declared_ret };
        let fn_type   = FunctionType {
            params: params.iter().map(|p| match p {
                BindingNode::IdentifierBinding { ty, .. } => self.compile_type(ty),
                _ => Type::Unknown,
            }).collect(),
            return_type: Box::new(final_ret),
        };

        if is_method { return (0, proto_local_idx, fn_type); }

        let cv        = ConstantValue::FunctionProto(proto_local_idx);
        let const_idx = self.add_constant(cv.clone());
        let dst       = self.reg_alloc.alloc();
        self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, const_idx as u32));

        if ctx.is_public {
            self.scopes.define_export(name.to_string(), dst, Type::Function(Box::new(fn_type.clone())), Some(cv), false);
        } else {
            self.scopes.define_local(name.to_string(), dst, Type::Function(Box::new(fn_type.clone())), Some(cv), false);
        }

        (dst, proto_local_idx, fn_type)
    }
}

impl LucyCompiler {
    fn offset_proto_idx(cv: &mut ConstantValue, offset: usize) {
        match cv {
            ConstantValue::FunctionProto(idx) => *idx += offset,
            ConstantValue::ClassProto { methods, .. } => {
                for (idx, _) in methods.iter_mut() { *idx += offset; }
            }
            _ => {}
        }
    }
}

impl LucyCompiler {
    pub fn find_namespace_in_scopes<'a>(scopes: &'a ScopeStack, name: &str) -> Option<&'a Namespace> {
        for scope in scopes.scopes.iter().rev() {
            if let Some(ns) = scope.namespaces.get(name) { return Some(ns); }
        }
        None
    }

    fn find_namespace_chained_static<'a>(scopes: &'a ScopeStack, parts: &[String]) -> Option<&'a Namespace> {
        if parts.is_empty() { return None; }
        let mut current = Self::find_namespace_in_scopes(scopes, &parts[0])?;
        for part in &parts[1..] {
            current = current.children.get(part)?;
        }
        Some(current)
    }

    pub fn collect_dot_chain(node: &AstNode, out: &mut Vec<String>) {
        match node {
            AstNode::Identifier(name) => { out.push(name.clone()); }
            AstNode::DotIndex { indexee, index } => {
                Self::collect_dot_chain(&indexee.node, out);
                if let AstNode::Identifier(name) = &index.node {
                    out.push(name.clone());
                }
            }
            _ => {}
        }
    }
}

impl LucyCompiler {
    fn compile_expr(&mut self, expr: &SAst, dst: usize, ctx: &CompilingCtx) {
        self.set_span(expr.span);

        match &expr.node {
            AstNode::IntLiteral(n) => {
                let k = self.add_constant(ConstantValue::I32(*n));
                self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
            }
            AstNode::FloatLiteral(f) => {
                let k = self.add_constant(ConstantValue::F64(*f));
                self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
            }
            AstNode::StringLiteral(s) => {
                let k = self.add_constant(ConstantValue::String(s.clone()));
                self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
            }
            
            AstNode::SelfExpr => {
                match self.scopes.resolve_local("self", self.proto_depth) {
                    Some(LocalResolution::Local { reg, .. }) => {
                        if reg != dst {
                            self.emit(pack_abc(Opcode::MOVE as u32, dst as u32, reg as u32, 0));
                        }
                    }
                    _ => self.compile_error("'self' used outside of a method"),
                }
            }

            AstNode::Borrowed(inner) => { self.compile_expr(inner, dst, ctx); }

            AstNode::MacroInvocation { name, args } => {
                let expanded = self.expand_macro_to_ast(name, args, expr.span);
                if expanded.len() != 1 {
                    panic!("macro used in expression position must expand to exactly 1 node, got {}", expanded.len());
                }
                self.compile_expr(&expanded[0], dst, ctx);
            }

            AstNode::Typeof(inner) => {
                let src = self.reg_alloc.alloc();
                self.compile_expr(inner, src, ctx);
                self.emit(pack_abc(Opcode::TYOF as u32, dst as u32, src as u32, 0));
                self.reg_alloc.free_to(src);
            }

            AstNode::Moved(inner) => {
                match &inner.node {
                    AstNode::Identifier(name) => {
                        match self.scopes.resolve_local(name, self.proto_depth) {
                            Some(LocalResolution::Local { moved: true, .. }) =>
                                self.compile_error(&format!("use of already-moved variable '{}'", name)),
                            Some(LocalResolution::OuterProto { .. }) =>
                                self.compile_error(&format!("cannot move '{}' captured from outer scope", name)),
                            None =>
                                self.compile_error(&format!("undefined variable '{}'", name)),
                            Some(LocalResolution::Local { reg, .. }) => {
                                if reg != dst {
                                    self.emit(pack_abc(Opcode::MOVE as u32, dst as u32, reg as u32, 0));
                                }
                                self.scopes.mark_moved(name);
                            }
                        }
                    }
                    _ => self.compile_expr(inner, dst, ctx),
                }
            }

            AstNode::Identifier(name) => {
                match self.scopes.resolve_local(name, self.proto_depth) {
                    Some(LocalResolution::Local { reg, moved, .. }) => {
                        if moved { self.compile_error(&format!("use of moved variable '{}'", name)); }
                        if reg != dst {
                            self.emit(pack_abc(Opcode::MOVE as u32, dst as u32, reg as u32, 0));
                        }
                    }
                    Some(LocalResolution::OuterProto { backing: Some(cv), .. }) => {
                        let k = self.add_constant(cv);
                        self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
                    }
                    Some(LocalResolution::OuterProto { reg, ty, .. }) => {
                        let uv_idx = self.capture_upvalue(name, UpvalueSource::ParentRegister(reg), ty);
                        self.emit(pack_abc(Opcode::GETUPVAL as u32, dst as u32, uv_idx as u32, 0));
                    }
                    None => self.compile_error(&format!("undefined variable '{}'", name)),
                }
            }

            AstNode::ClassLiteral { ty, fields } => {
                let class_name = match &ty.node {
                    AstNode::Identifier(s) => s.clone(),
                    AstNode::SelfExpr => ctx.current_class.clone()
                        .unwrap_or_else(|| self.compile_error("Self used outside of class")),
                    AstNode::Borrowed(b) | AstNode::Moved(b) => match &b.node {
                        AstNode::Identifier(s) => s.clone(),
                        AstNode::SelfExpr => ctx.current_class.clone()
                            .unwrap_or_else(|| self.compile_error("Self used outside of class")),
                        _ => self.compile_error("Unknown class type in literal"),
                    },
                    _ => self.compile_error("Unknown class type in literal"),
                };

                let class_ty_opt = self.scopes.lookup_type(&class_name).cloned();
                let proto_k = {
                    let class_id = match &class_ty_opt {
                        Some(Type::Class(id)) => *id,
                        _ => self.compile_error(&format!("'{}' is not a class type", class_name)),
                    };
                    let cv = self.type_arena.get_class(class_id)
                        .class_proto_constant.clone()
                        .unwrap_or_else(|| self.compile_error(&format!(
                            "ClassProto not built for '{}'", class_name
                        )));
                    self.add_constant(cv)
                };
                self.emit(pack_abx(Opcode::NEWCLASS as u32, dst as u32, proto_k as u32));

                for (fname, fexpr) in fields {
                    if let Some(Type::Class(id)) = &class_ty_opt {
                        let class = self.type_arena.get_class(*id);
                        if !class.fields.iter().any(|(n, _, _)| n == fname) {
                            self.compile_error(&format!(
                                "class '{}' has no field '{}'", class_name, fname
                            ));
                        }
                    }
                    let saved   = self.reg_alloc.current_top;
                    let val_reg = self.reg_alloc.alloc();
                    self.compile_expr(fexpr, val_reg, ctx);
                    let field_index = match &class_ty_opt {
                        Some(Type::Class(id)) => {
                            *self.type_arena.get_class(*id).field_index_map.get(fname)
                                .unwrap_or_else(|| self.compile_error(&format!("unknown field '{}'", fname)))
                        }
                        _ => self.compile_error("Expected class type"),
                    };
                    self.emit(pack_abc(
                        Opcode::SETFIELD as u32, dst as u32, val_reg as u32, field_index as u32,
                    ));
                    self.reg_alloc.free_to(saved);
                }
            }

            AstNode::FunctionCall { callee, args } => {
                let callee_name = match &callee.node {
                    AstNode::Identifier(s) => Some(s.clone()),
                    _ => None,
                };

                let is_variadic = callee_name.as_ref()
                    .map(|n| self.variadic_templates.contains_key(n))
                    .unwrap_or(false);

                if is_variadic {
                    let name         = callee_name.unwrap();
                    let fixed_count  = self.variadic_templates[&name].fixed_params.len();
                    let vararg_count = if args.len() > fixed_count {
                        if args.last().map(|a| matches!(a.node, AstNode::Ellipsis)).unwrap_or(false) {
                            self.current_vararg_regs.len()
                        } else {
                            args.len() - fixed_count
                        }
                    } else { 0 };

                    let mangled = format!("{}#{}", name, vararg_count);

                    if !self.completed_specializations.contains(&(name.clone(), vararg_count)) {
                        self.pending_specializations.push(PendingSpecialization {
                            name:         name.clone(),
                            vararg_count,
                            ctx:          ctx.clone(),
                        });
                    }

                    let cv        = ConstantValue::String(mangled.clone());
                    let const_idx = self.add_constant(cv);
                    self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, const_idx as u32));

                    self.reg_alloc.current_top = dst + 1;

                    let template     = self.variadic_templates[&name].clone();
                    let fixed_count  = template.fixed_params.len();

                    for arg in &args[..fixed_count.min(args.len())] {
                        let arg_reg = self.reg_alloc.alloc();
                        let saved   = self.reg_alloc.current_top;
                        self.compile_expr(arg, arg_reg, ctx);
                        self.reg_alloc.current_top = saved;
                    }

                    if args.last().map(|a| matches!(a.node, AstNode::Ellipsis)).unwrap_or(false) {
                        for &vreg in &self.current_vararg_regs.clone() {
                            let arg_reg = self.reg_alloc.alloc();
                            self.emit(pack_abc(Opcode::MOVE as u32, arg_reg as u32, vreg as u32, 0));
                        }
                    } else {
                        for arg in &args[fixed_count..] {
                            let arg_reg = self.reg_alloc.alloc();
                            let saved   = self.reg_alloc.current_top;
                            self.compile_expr(arg, arg_reg, ctx);
                            self.reg_alloc.current_top = saved;
                        }
                    }

                    self.emit(pack_abc(
                        Opcode::CALL as u32,
                        dst as u32,
                        args.len() as u32,
                        1,
                    ));
                    self.reg_alloc.current_top = dst + 1;
                    return;
                }

                self.compile_expr(callee, dst, ctx);

                let implicit_self = matches!(&callee.node, AstNode::MethodCall { .. });

                self.reg_alloc.current_top = if implicit_self { dst + 2 } else { dst + 1 };

                for arg in args.iter() {
                    let arg_reg = self.reg_alloc.alloc();
                    let saved   = self.reg_alloc.current_top;
                    self.compile_expr(arg, arg_reg, ctx);
                    self.reg_alloc.current_top = saved;
                }

                self.emit(pack_abc(
                    Opcode::CALL as u32,
                    dst as u32,
                    (args.len() + if implicit_self { 1 } else { 0 }) as u32,
                    1,
                ));

                self.reg_alloc.current_top = dst + 1;
            }

            AstNode::DotIndex { indexee, index } => {
                let mut parts = Vec::new();
                Self::collect_dot_chain(&indexee.node, &mut parts);

                let member_name = match &index.node {
                    AstNode::Identifier(s) => s.clone(),
                    other => self.compile_error(&format!("DotIndex: expected ident, got {:?}", other)),
                };

                let ns_owner = parts.last().cloned().unwrap_or_default();

                let found = Self::find_namespace_chained_static(&self.scopes, &parts)
                    .and_then(|ns| ns.locals.get(&member_name).map(|(_, is_pub)| *is_pub))
                    .or_else(|| {
                        Self::find_namespace_in_scopes(&self.scopes, &ns_owner)
                            .and_then(|ns| ns.locals.get(&member_name).map(|(_, is_pub)| *is_pub))
                    });

                let is_public      = found.unwrap_or(true);
                let is_inside_class = ctx.current_class.as_deref() == Some(&ns_owner);

                if !is_public && !is_inside_class {
                    self.compile_error(&format!("method '{}.{}' is private", ns_owner, member_name));
                }

                let qualified = format!("{}::{}", ns_owner, member_name);

                match self.scopes.resolve_local(&qualified, self.proto_depth) {
                    Some(LocalResolution::Local { reg, .. }) => {
                        if reg != dst {
                            self.emit(pack_abc(Opcode::MOVE as u32, dst as u32, reg as u32, 0));
                        }
                    }
                    Some(LocalResolution::OuterProto { backing: Some(cv), .. }) => {
                        let k = self.add_constant(cv);
                        self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
                    }
                    _ => {
                        let cv_opt = Self::find_namespace_chained_static(&self.scopes, &parts)
                            .and_then(|ns| ns.constants.get(&member_name).cloned())
                            .or_else(|| {
                                Self::find_namespace_in_scopes(&self.scopes, &ns_owner)
                                    .and_then(|ns| ns.constants.get(&member_name).cloned())
                            });

                        match cv_opt {
                            Some(cv) => {
                                let k = self.add_constant(cv);
                                self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
                            }
                            None => {
                                let obj = self.reg_alloc.alloc();
                                self.compile_expr(indexee, obj, ctx);

                                let obj_ty = self.infer_expr_type(&indexee.node, ctx);
                                let field_idx = match obj_ty {
                                    Type::Class(id) => {
                                        let class = self.type_arena.get_class(id);
                                        match class.field_index_map.get(&member_name) {
                                            Some(idx) => *idx,
                                            None => self.compile_error(&format!(
                                                "unknown field '{}'", member_name
                                            )),
                                        }
                                    }
                                    _ => self.compile_error("attempt to index non-class"),
                                };

                                self.emit(pack_abc(
                                    Opcode::GETFIELD as u32, dst as u32, obj as u32, field_idx as u32,
                                ));
                            }
                        }
                    }
                }
            }

            AstNode::BinaryOperation { op, left, right } => {
                let lt = self.infer_expr_type(&left.node, ctx);
                let rt = self.infer_expr_type(&right.node, ctx);
                let l_ty = lt.inner().clone();
                let r_ty = rt.inner().clone();

                let saved = self.reg_alloc.current_top;
                let l_reg = self.reg_alloc.alloc();
                let r_reg = self.reg_alloc.alloc();
                self.compile_expr(left,  l_reg, ctx);
                self.compile_expr(right, r_reg, ctx);

                let overload = match (&l_ty, &r_ty) {
                    (Type::Class(id), _) | (_, Type::Class(id)) => {
                        self.type_arena.get_class(*id).operators.get(op).cloned()
                    }
                    _ => None,
                };
                let is_overloaded = overload.is_some();

                let vm_op = match (op, is_overloaded) {
                    (Operator::Add, true)  => Opcode::ADDOV,
                    (Operator::Sub, true)  => Opcode::SUBOV,
                    (Operator::Mul, true)  => Opcode::MULOV,
                    (Operator::Div, true)  => Opcode::DIVOV,
                    (Operator::Add, false) => Opcode::ADD,
                    (Operator::Sub, false) => Opcode::SUB,
                    (Operator::Mul, false) => Opcode::MUL,
                    (Operator::Div, false) => Opcode::DIV,
                    (Operator::Mod, false) => Opcode::MOD,
                    (Operator::Pow, false) => Opcode::POW,
                    (Operator::BLShift, false) => Opcode::BLSHIFT,
                    (Operator::BRShift, false) => Opcode::BRSHIFT,
                    (Operator::BAnd, false) => Opcode::BAND,
                    (Operator::BOr,  false) => Opcode::BOR,
                    (Operator::Lt,   false) => Opcode::LT,
                    (Operator::Gt,   false) => Opcode::GT,
                    (Operator::Le,   false) => Opcode::LE,
                    (Operator::Ge,   false) => Opcode::GE,
                    (Operator::Eq,   false) => Opcode::EQ,
                    (Operator::NEq,  false) => Opcode::NEQ,
                    _ => self.compile_error(&format!("Unhandled operator {:?}", op)),
                };

                self.emit(pack_abc(vm_op as u32, dst as u32, l_reg as u32, r_reg as u32));
                self.reg_alloc.free_to(saved);
            }

            AstNode::MethodCall { indexee, index } => {
                let obj = self.reg_alloc.alloc();
                self.compile_expr(indexee, obj, ctx);

                let method_name = match &index.node {
                    AstNode::Identifier(s) => s.clone(),
                    other => self.compile_error(&format!("MethodCall: expected ident, got {:?}", other)),
                };

                let obj_ty = self.infer_expr_type(&indexee.node, ctx);
                let method_idx = match obj_ty {
                    Type::Class(id) => {
                        let class = self.type_arena.get_class(id);
                        match class.methods.get(&method_name) {
                            Some((idx, _, _, _)) => *idx,
                            None => self.compile_error(&format!("unknown method '{}'", method_name)),
                        }
                    }
                    _ => self.compile_error("attempt to call method on non-class"),
                };

                self.emit(pack_abc(
                    Opcode::GETMETHOD as u32, dst as u32, obj as u32, method_idx as u32,
                ));
            }

            // AstNode::WhileLoop { condition, body } => {
            //     self.compile_while(condition, body, ctx);
            //     let k = self.add_constant(ConstantValue::)
            //     self.emit(pack_abx(Opcode::LOADK as u32, dst as u32, k as u32));
            // }

            // AstNode::ForLoop { binding, iterator, body } => {
            //     self.compile_for(binding, iterator, body, ctx);
            // }

            AstNode::TypeCast { left, right } => {
                let target_ty = self.compile_type(right);
                let src_reg   = self.reg_alloc.alloc();
                self.compile_expr(left, src_reg, ctx);
                let ty_const = self.add_constant(ConstantValue::Type(target_ty));
                self.emit(pack_abc(Opcode::TYCAST as u32, dst as u32, src_reg as u32, ty_const as u32));
                self.reg_alloc.free_to(src_reg);
            }

            AstNode::Block(stmts) => {
                self.enter_scope();
                if !stmts.is_empty() {
                    let (body, tail) = stmts.split_at(stmts.len() - 1);
                    for s in body { self.compile_stmt(s, ctx); }
                    self.compile_expr(&tail[0], dst, ctx);
                }
                self.exit_scope();
            }

            AstNode::ConditionalBranch { condition, body, else_body } => {
                let saved    = self.reg_alloc.current_top;
                let cond_reg = self.reg_alloc.alloc();
                self.compile_expr(condition, cond_reg, ctx);

                let false_reg = self.reg_alloc.alloc();
                let k = self.add_constant(ConstantValue::I32(0));
                self.emit(pack_abx(Opcode::LOADK as u32, false_reg as u32, k as u32));

                let jump_over_body = self.current_proto().code.len();
                self.emit(pack_abc(Opcode::JEQ as u32, 128, cond_reg as u32, false_reg as u32));
                self.reg_alloc.free_to(saved);

                self.compile_block_into(body, dst, ctx);

                let jump_over_else = self.current_proto().code.len();
                self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));

                let else_start = self.current_proto().code.len();
                self.patch_jmp(jump_over_body, else_start);

                if let Some(e) = else_body {
                    self.compile_block_into(e, dst, ctx);
                }

                let end_pc = self.current_proto().code.len();
                self.patch_jmp(jump_over_else, end_pc);
            }

            AstNode::MatchStmt { matchee, arms } => {
                let saved       = self.reg_alloc.current_top;
                let matchee_reg = self.reg_alloc.alloc();
                self.compile_expr(matchee, matchee_reg, ctx);

                let mut exit_jumps: Vec<usize> = Vec::new();

                for arm in arms {
                    match &arm.pattern {
                        MatchPattern::Expr(pat_expr) => {
                            let pat_reg = self.reg_alloc.alloc();
                            self.compile_expr(pat_expr, pat_reg, ctx);

                            let skip_jump = self.current_proto().code.len();
                            self.emit(pack_abc(Opcode::JNE as u32, 128, matchee_reg as u32, pat_reg as u32));
                            self.reg_alloc.free_to(saved + 1);

                            self.compile_block_into(&arm.body, dst, ctx);

                            let exit_jump = self.current_proto().code.len();
                            self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));
                            exit_jumps.push(exit_jump);

                            let next_arm = self.current_proto().code.len();
                            self.patch_jmp(skip_jump, next_arm);
                        }
                        MatchPattern::Binding(BindingNode::IdentifierBinding { name, ty }) => {
                            self.enter_scope();
                            let bind_reg = self.reg_alloc.alloc();
                            self.emit(pack_abc(Opcode::MOVE as u32, bind_reg as u32, matchee_reg as u32, 0));
                            let compiled_ty = self.compile_type(ty);
                            self.scopes.define_local(name.clone(), bind_reg, compiled_ty, None, false);

                            self.compile_block_into(&arm.body, dst, ctx);
                            self.exit_scope();

                            let exit_jump = self.current_proto().code.len();
                            self.emit(pack_abc(Opcode::JMP as u32, 128, 0, 0));
                            exit_jumps.push(exit_jump);
                        }
                        other => panic!("Unhandled match pattern: {:?}", other),
                    }
                }

                let end_pc = self.current_proto().code.len();
                for jump_idx in exit_jumps { self.patch_jmp(jump_idx, end_pc); }
                self.reg_alloc.free_to(saved);
            }

            other => self.compile_error(&format!("Unhandled expression node: {:?}", other)),
        }
    }
}

impl LucyCompiler {
    fn expand_macro_to_ast(&mut self, name: &str, args: &Vec<MacroTokenTree>, _span: Span) -> Vec<SAst> {
        let expanded = self.expand_macro(name, args);
        let tokens   = self.flatten_tts_to_tokens(&expanded);
        let mut parser = LucyParser::new(tokens);
        let parsed     = parser.parse_file_source();
        match parsed.node {
            AstNode::Program(stmts) => stmts,
            _                       => vec![parsed],
        }
    }

    fn expand_macro(&self, name: &str, args: &[MacroTokenTree]) -> Vec<MacroTokenTree> {
        let mac = self.macros.get(name)
            .unwrap_or_else(|| panic!("macro '{}' not found", name));
        for arm in &mac.arms {
            let mut bindings = HashMap::new();
            if self.match_trees(&arm.pattern, args, &mut bindings) {
                return self.expand_trees(&arm.template, &bindings);
            }
        }
        panic!("no matching macro arm for '{}'", name);
    }

    fn expand_trees(
        &self,
        template: &[MacroTokenTree],
        bindings: &HashMap<String, Vec<MacroTokenTree>>,
    ) -> Vec<MacroTokenTree> {
        let mut out = Vec::new();
        for t in template {
            match t {
                MacroTokenTree::Token(tok, span) => {
                    out.push(MacroTokenTree::Token(tok.clone(), *span));
                }
                MacroTokenTree::Metavar { name, .. } => {
                    if let Some(v) = bindings.get(name) {
                        out.extend(v.iter().cloned());
                    }
                }
                MacroTokenTree::Group { delimiter, trees } => {
                    out.push(MacroTokenTree::Group {
                        delimiter: *delimiter,
                        trees:     self.expand_trees(trees, bindings),
                    });
                }
                MacroTokenTree::Repetition { trees, separator, quantifier } => {
                    let count = self.rep_capture_count(trees, bindings);
                    for rep_i in 0..count {
                        if rep_i > 0 {
                            if let Some(sep) = separator {
                                out.push(MacroTokenTree::Token(str_to_sep_token(sep), Span::dummy()));
                            }
                        }
                        let slot: HashMap<String, Vec<MacroTokenTree>> = bindings.iter().map(|(k, v)| {
                            let elem = v.get(rep_i).cloned()
                                .or_else(|| v.last().cloned())
                                .unwrap_or_else(|| MacroTokenTree::Token(Token::IDENT("_"), Span::dummy()));
                            (k.clone(), vec![elem])
                        }).collect();
                        out.extend(self.expand_trees(trees, &slot));
                    }
                }
            }
        }
        out
    }

    fn match_trees(
        &self,
        pattern:  &[MacroTokenTree],
        input:    &[MacroTokenTree],
        bindings: &mut HashMap<String, Vec<MacroTokenTree>>,
    ) -> bool {
        match self.match_seq(pattern, input, bindings) {
            Some(consumed) => consumed == input.len(),
            None => false,
        }
    }

    fn match_seq(
        &self,
        pattern:  &[MacroTokenTree],
        input:    &[MacroTokenTree],
        bindings: &mut HashMap<String, Vec<MacroTokenTree>>,
    ) -> Option<usize> {
        let mut i = 0;
        let mut j = 0;
        while i < pattern.len() {
            match &pattern[i] {
                MacroTokenTree::Token(expected, _) => {
                    match input.get(j) {
                        Some(MacroTokenTree::Token(actual, _)) if tokens_equal(actual, expected) => {
                            i += 1; j += 1;
                        }
                        _ => return None,
                    }
                }
                MacroTokenTree::Metavar { name, designator } => {
                    let captured = match input.get(j) {
                        Some(tt) => tt.clone(),
                        None     => return None,
                    };
                    if !designator_matches(designator, &captured) { return None; }
                    bindings.entry(name.clone()).or_default().push(captured);
                    i += 1; j += 1;
                }
                MacroTokenTree::Group { delimiter, trees } => {
                    match input.get(j) {
                        Some(MacroTokenTree::Group { delimiter: d2, trees: inner }) if d2 == delimiter => {
                            let mut sub = HashMap::new();
                            match self.match_seq(trees, inner, &mut sub) {
                                Some(consumed) if consumed == inner.len() => {}
                                _ => return None,
                            }
                            for (k, v) in sub { bindings.entry(k).or_default().extend(v); }
                            i += 1; j += 1;
                        }
                        _ => return None,
                    }
                }
                MacroTokenTree::Repetition { trees, separator, quantifier } => {
                    let mut count     = 0;
                    let mut input_pos = j;
                    let mut rep_bindings: HashMap<String, Vec<MacroTokenTree>> = HashMap::new();
                    loop {
                        let mut trial = HashMap::new();
                        match self.match_seq(trees, &input[input_pos..], &mut trial) {
                            None | Some(0) => break,
                            Some(consumed) => {
                                for (k, v) in trial { rep_bindings.entry(k).or_default().extend(v); }
                                input_pos += consumed;
                                count     += 1;
                                if let Some(sep_str) = separator {
                                    match input.get(input_pos) {
                                        Some(MacroTokenTree::Token(tok, _)) if token_matches_sep(tok, sep_str) => {
                                            input_pos += 1;
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                    }
                    if matches!(quantifier, RepQuantifier::OneOrMore) && count == 0 { return None; }
                    for (k, v) in rep_bindings { bindings.entry(k).or_default().extend(v); }
                    i += 1; j = input_pos;
                }
            }
        }
        Some(j)
    }

    fn flatten_tts_to_tokens(&self, tts: &[MacroTokenTree]) -> Vec<(Token<'static>, Span)> {
        let mut out = Vec::new();
        for t in tts {
            match t {
                MacroTokenTree::Token(tok, span) => { out.push((tok.clone(), *span)); }
                MacroTokenTree::Group { delimiter, trees } => {
                    let (open, close) = delimiter_tokens(*delimiter);
                    out.push((open,  Span::dummy()));
                    out.extend(self.flatten_tts_to_tokens(trees));
                    out.push((close, Span::dummy()));
                }
                MacroTokenTree::Metavar { name, .. } =>
                    panic!("Unexpanded metavar '{}' after expansion", name),
                MacroTokenTree::Repetition { .. } =>
                    panic!("Unexpanded repetition after expansion"),
            }
        }
        out
    }

    fn rep_capture_count(&self, trees: &[MacroTokenTree], bindings: &HashMap<String, Vec<MacroTokenTree>>) -> usize {
        for t in trees {
            match t {
                MacroTokenTree::Metavar { name, .. } => {
                    if let Some(v) = bindings.get(name) { return v.len(); }
                }
                MacroTokenTree::Group { trees: inner, .. } => {
                    let n = self.rep_capture_count(inner, bindings);
                    if n > 0 { return n; }
                }
                _ => {}
            }
        }
        0
    }
}

fn tokens_equal(a: &Token<'static>, b: &Token<'static>) -> bool {
    match (a, b) {
        (Token::IDENT(x),  Token::IDENT(y))  => x == y,
        (Token::BINOP(x),  Token::BINOP(y))  => x == y,
        (Token::UNARY(x),  Token::UNARY(y))  => x == y,
        (Token::PUNCT(x),  Token::PUNCT(y))  => x == y,
        (Token::INT(x),    Token::INT(y))     => x == y,
        (Token::FLOAT(x),  Token::FLOAT(y))  => x.to_bits() == y.to_bits(),
        (Token::STRING(x), Token::STRING(y)) => x == y,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

fn designator_matches(des: &MacroDesignator, tt: &MacroTokenTree) -> bool {
    match des {
        MacroDesignator::Tt      => true,
        MacroDesignator::Str     => matches!(tt, MacroTokenTree::Token(Token::STRING(_), _)),
        MacroDesignator::Int     => matches!(tt, MacroTokenTree::Token(Token::INT(_),    _)),
        MacroDesignator::Float   => matches!(tt, MacroTokenTree::Token(Token::FLOAT(_),  _)),
        MacroDesignator::Literal => matches!(tt,
            MacroTokenTree::Token(Token::STRING(_), _)
            | MacroTokenTree::Token(Token::INT(_),  _)
            | MacroTokenTree::Token(Token::FLOAT(_), _)),
        MacroDesignator::Ident   => matches!(tt, MacroTokenTree::Token(Token::IDENT(_), _)),
        _                        => true,
    }
}

fn str_to_sep_token(sep: &str) -> Token<'static> {
    match sep {
        "," => Token::PUNCT(","),
        ";" => Token::PUNCT(";"),
        "|" => Token::BINOP("|"),
        "+" => Token::BINOP("+"),
        other => {
            let s: &'static str = Box::leak(other.to_string().into_boxed_str());
            Token::PUNCT(s)
        }
    }
}

fn token_matches_sep(tok: &Token<'static>, sep: &str) -> bool {
    match tok {
        Token::PUNCT(s) | Token::BINOP(s) | Token::UNARY(s) => *s == sep,
        Token::IDENT(s) => *s == sep,
        _ => false,
    }
}

fn delimiter_tokens(delim: char) -> (Token<'static>, Token<'static>) {
    match delim {
        '(' => (Token::PAREN("("), Token::PAREN(")")),
        '[' => (Token::PAREN("["), Token::PAREN("]")),
        '{' => (Token::PAREN("{"), Token::PAREN("}")),
        other => panic!("unknown group delimiter '{}'", other),
    }
}

use std::fmt;
impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::U8     => write!(f, "u8"),     Type::I8    => write!(f, "i8"),
            Type::U16    => write!(f, "u16"),    Type::I16   => write!(f, "i16"),
            Type::U32    => write!(f, "u32"),    Type::I32   => write!(f, "i32"),
            Type::U64    => write!(f, "u64"),    Type::I64   => write!(f, "i64"),
            Type::F32    => write!(f, "f32"),    Type::F64   => write!(f, "f64"),
            Type::USize  => write!(f, "usize"),  Type::Bool  => write!(f, "bool"),
            Type::String => write!(f, "string"), Type::Empty => write!(f, "empty"),
            Type::Unknown => write!(f, "<inferred>"),
            Type::Array(inner) => write!(f, "[{}]", inner),
            Type::TypeVar(n)   => write!(f, "{}", n),
            Type::Class(id)    => write!(f, "{:?}", id),
            Type::Function(ft) => {
                write!(f, "fn(")?;
                for (i, p) in ft.params.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ft.return_type)
            }
            Type::Qualified { inner, mutable, borrowed, moved } => {
                if *mutable  { write!(f, "mut ")?; }
                if *borrowed { write!(f, "&")?; }
                if *moved    { write!(f, "move ")?; }
                write!(f, "{}", inner)
            }
            Type::Generic { name, args } => {
                write!(f, "{}<", name)?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", a)?;
                }
                write!(f, ">")
            }
        }
    }
}