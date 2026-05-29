#![allow(unused)]

use std::collections::{HashMap, HashSet};

use crate::parser::{
    AstNode, BindingNode, ClassMember, MatchArm, MatchPattern,
    SAst, TypeNode,
};

use crate::span::Span;

use crate::operator::Operator;

use crate::ty::{
    ClassType, FunctionType, Type, TypeArena,
};

#[derive(Debug, Clone)]
pub struct Local {
    pub ty:      Type,
    pub mutable: bool,
    pub moved:   bool,
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub children: HashMap<String, Namespace>,
    pub locals:   HashMap<String, Type>,
    pub types:    HashMap<String, Type>,
}

impl Namespace {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
            locals:   HashMap::new(),
            types:    HashMap::new(),
        }
    }
}

pub struct NamespaceBuilder {
    namespace: Namespace,
}

impl NamespaceBuilder {
    pub fn new() -> Self {
        Self { namespace: Namespace::new() }
    }

    pub fn construct(
        build: impl FnOnce(NamespaceBuilder) -> NamespaceBuilder,
    ) -> Namespace
    {
        build(NamespaceBuilder::new()).build()
    }

    pub fn member(
        mut self,
        name:  &str,
        ty: Type,
    ) -> Self {
        self.namespace.locals.insert(name.to_string(), ty);
        self
    }

    pub fn build(self) -> Namespace { self.namespace }
}

#[derive(Debug)]
pub struct Scope {
    pub locals:     HashMap<String, Local>,
    pub types:      HashMap<String, Type>,
    pub namespaces: HashMap<String, Namespace>,
}

#[derive(Debug)]
pub struct ScopeStack {
    pub scopes: Vec<Scope>,
}

impl ScopeStack {
    pub fn new() -> Self {
        Self { scopes: vec![] }
    }

    pub fn push(&mut self) {
        self.scopes.push(Scope {
            locals:     HashMap::new(),
            types:      HashMap::new(),
            namespaces: HashMap::new(),
        });
    }

    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    pub fn define_local(&mut self, name: String, ty: Type, mutable: bool) {
        self.scopes.last_mut().unwrap().locals.insert(
            name,
            Local { ty, mutable, moved: false },
        );
    }

    pub fn define_type(&mut self, name: String, ty: Type) {
        self.scopes.last_mut().unwrap().types.insert(name, ty);
    }

    pub fn define_namespace(&mut self, name: String, ns: Namespace) {
        self.scopes.last_mut().unwrap().namespaces.insert(name, ns);
    }

    pub fn resolve_local(&self, name: &str) -> Option<&Local> {
        for scope in self.scopes.iter().rev() {
            if let Some(local) = scope.locals.get(name) {
                return Some(local);
            }
        }
        None
    }

    pub fn resolve_local_mut(&mut self, name: &str) -> Option<&mut Local> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.locals.get_mut(name) {
                return Some(local);
            }
        }
        None
    }

    pub fn resolve_type(&self, name: &str) -> Option<&Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.types.get(name) {
                return Some(ty);
            }
        }
        None
    }

    pub fn resolve_namespace(&self, name: &str) -> Option<&Namespace> {
        for scope in self.scopes.iter().rev() {
            if let Some(ns) = scope.namespaces.get(name) {
                return Some(ns);
            }
        }
        None
    }

    pub fn mark_moved(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.locals.get_mut(name) {
                local.moved = true;
                return;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeError {
    pub span:    Span,
    pub message: String,
}

pub struct TypeChecker {
    pub scopes:        ScopeStack,
    pub type_arena:    TypeArena,
    pub errors:        Vec<TypeError>,
    pub current_class: Option<String>,
    /// Return type of the function currently being checked. `Type::Unknown`
    /// means no function is in scope or the return type was not declared.
    current_return_ty: Type,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut scopes = ScopeStack::new();
        scopes.push();

        Self {
            scopes,
            type_arena:        TypeArena::new(),
            errors:            vec![],
            current_class:     None,
            current_return_ty: Type::Unknown,
        }
    }

    pub fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(TypeError { span, message: msg.into() });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Entry point
    // ─────────────────────────────────────────────────────────────────────────

    pub fn check_program(&mut self, program: &SAst) {
        self.check_stmt(program);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Statement checking
    // ─────────────────────────────────────────────────────────────────────────

    pub fn check_stmt(&mut self, stmt: &SAst) {
        match &stmt.node {
            AstNode::Program(stmts) => {
                for s in stmts { self.check_stmt(s); }
            }

            AstNode::ModuleStmt { .. } => {}

            AstNode::Public(inner) => {
                self.check_stmt(inner);
            }

            AstNode::UseStmt { base_path, used } => {
                // Resolve and clone the namespace fully before any mutation.
                let ns_opt = self.resolve_namespace_path(&base_path.node).cloned();
                match ns_opt {
                    None => {
                        self.error(stmt.span, "unknown namespace");
                    }
                    Some(ns) => {
                        // Collect what we need before touching self.
                        let mut to_define_locals: Vec<(String, Type)> = Vec::new();
                        let mut to_define_types:  Vec<(String, Type)> = Vec::new();
                        let mut unknown_imports:  Vec<String>          = Vec::new();

                        for (actual, alias) in used {
                            if let Some(ty) = ns.locals.get(actual) {
                                to_define_locals.push((alias.clone(), ty.clone()));
                            } else if let Some(ty) = ns.types.get(actual) {
                                to_define_types.push((alias.clone(), ty.clone()));
                            } else {
                                unknown_imports.push(actual.clone());
                            }
                        }

                        for name in unknown_imports {
                            self.error(stmt.span, format!("unknown import '{}'", name));
                        }
                        for (alias, ty) in to_define_locals {
                            self.scopes.define_local(alias, ty, false);
                        }
                        for (alias, ty) in to_define_types {
                            self.scopes.define_type(alias, ty);
                        }
                    }
                }
            }

            AstNode::VarDeclaration { binding, init_value } => {
                match binding {
                    BindingNode::IdentifierBinding { name, ty } => {
                        let declared_ty = self.compile_type(ty);

                        let inferred = match init_value {
                            Some(v) => self.infer_expr_type(v),
                            None    => Type::Unknown,
                        };

                        let final_ty = if matches!(declared_ty, Type::Unknown) {
                            inferred.clone()
                        } else {
                            declared_ty.clone()
                        };

                        if !matches!(declared_ty, Type::Unknown) && !matches!(inferred, Type::Unknown) {
                            self.assert_assignable(&declared_ty, &inferred, stmt.span);
                        }

                        self.scopes.define_local(
                            name.clone(),
                            final_ty,
                            declared_ty.is_mutable(),
                        );
                    }
                    _ => {}
                }
            }

            AstNode::Assignment { left, right } => {
                let rhs_ty = self.infer_expr_type(right);

                match &left.node {
                    AstNode::Identifier(name) => {
                        // Clone what we need before any mutable borrow.
                        let local_info = self.scopes.resolve_local(name)
                            .map(|l| (l.mutable, l.ty.clone()));

                        match local_info {
                            Some((mutable, ty)) => {
                                if !mutable {
                                    self.error(stmt.span, format!(
                                        "cannot assign to immutable variable '{}'", name
                                    ));
                                }
                                self.assert_assignable(&ty, &rhs_ty, stmt.span);
                            }
                            None => {
                                self.error(stmt.span, format!("undefined variable '{}'", name));
                            }
                        }
                    }
                    AstNode::DotIndex { indexee, index } => {
                        let obj_ty     = self.infer_expr_type(indexee);
                        let field_name = match &index.node {
                            AstNode::Identifier(s) => s.clone(),
                            _ => return,
                        };
                        // Clone field type out of arena before calling self.error / assert_assignable.
                        let field_result = match &obj_ty {
                            Type::Class(id) => {
                                let class = self.type_arena.get_class(*id);
                                match class.fields.iter().find(|(n, _, _)| n == &field_name) {
                                    Some((_, ty, _)) => Ok(ty.clone()),
                                    None             => Err(format!("class has no field '{}'", field_name)),
                                }
                            }
                            _ => return,
                        };
                        match field_result {
                            Ok(field_ty) => self.assert_assignable(&field_ty, &rhs_ty, stmt.span),
                            Err(msg)     => self.error(stmt.span, msg),
                        }
                    }
                    _ => {}
                }
            }

            AstNode::FunctionDeclaration { name, params, return_type, body, .. } => {
                let fn_ty = self.build_fn_type(params, return_type);

                self.scopes.define_local(
                    name.clone(),
                    Type::Function(Box::new(fn_ty.clone())),
                    false,
                );

                let saved_ret          = self.current_return_ty.clone();
                self.current_return_ty = *fn_ty.return_type.clone();

                self.scopes.push();
                self.register_params(params);
                for s in body { self.check_stmt(s); }
                self.scopes.pop();

                self.current_return_ty = saved_ret;
            }

            AstNode::ClassDefinition { name, members } => {
                self.check_class_definition(name, members, stmt.span);
            }

            AstNode::ConditionalBranch { condition, body, next } => {
                if let Some(cond) = condition { self.infer_expr_type(cond); }
                self.scopes.push();
                for s in body { self.check_stmt(s); }
                self.scopes.pop();
                if let Some(next) = next { self.check_stmt(next); }
            }

            AstNode::WhileLoop { condition, body } => {
                self.infer_expr_type(condition);
                self.scopes.push();
                for s in body { self.check_stmt(s); }
                self.scopes.pop();
            }

            AstNode::ForLoop { binding, iterator, body } => {
                self.infer_expr_type(iterator);
                self.scopes.push();
                if let BindingNode::IdentifierBinding { name, ty } = binding {
                    let compiled = self.compile_type(ty);
                    self.scopes.define_local(name.clone(), compiled, false);
                }
                for s in body { self.check_stmt(s); }
                self.scopes.pop();
            }

            AstNode::MatchStmt { matchee, arms } => {
                let matchee_ty = self.infer_expr_type(matchee);
                for arm in arms {
                    self.scopes.push();
                    match &arm.pattern {
                        MatchPattern::Expr(expr) => {
                            let pat_ty = self.infer_expr_type(expr);
                            self.assert_assignable(&matchee_ty, &pat_ty, expr.span);
                        }
                        MatchPattern::Binding(BindingNode::IdentifierBinding { name, ty }) => {
                            let compiled = self.compile_type(ty);
                            self.scopes.define_local(name.clone(), compiled, false);
                        }
                        _ => {}
                    }
                    for s in &arm.body { self.check_stmt(s); }
                    self.scopes.pop();
                }
            }

            AstNode::ReturnStmt { value } => {
                match value {
                    Some(expr) => {
                        let ret_ty        = self.infer_expr_type(expr);
                        let expected      = self.current_return_ty.clone();
                        if !matches!(expected, Type::Unknown) {
                            self.assert_assignable(&expected, &ret_ty, expr.span);
                        }
                    }
                    None => {
                        let expected = self.current_return_ty.clone();
                        if !matches!(expected, Type::Unknown | Type::Empty) {
                            self.error(stmt.span, format!(
                                "bare return in function expecting '{}'", expected
                            ));
                        }
                    }
                }
            }

            AstNode::FunctionCall { callee, args } => {
                self.check_call(callee, args, stmt.span);
            }

            AstNode::BinaryOperation { left, right, op } => {
                let lt = self.infer_expr_type(left);
                let rt = self.infer_expr_type(right);
                // Clone overload info out of the arena before any further borrows.
                let overload = match (lt.inner(), rt.inner()) {
                    (Type::Class(id), _) | (_, Type::Class(id)) => {
                        self.type_arena.get_class(*id).operators.get(op).cloned()
                    }
                    _ => None,
                };
                if let Some((_, fn_ty)) = overload {
                    if fn_ty.params.len() == 2 {
                        let p0 = fn_ty.params[0].clone();
                        let p1 = fn_ty.params[1].clone();
                        self.assert_assignable(&p0, lt.inner(), left.span);
                        self.assert_assignable(&p1, rt.inner(), right.span);
                    } else {
                        self.error(left.span, format!(
                            "operator '{:?}' overload must take exactly 2 parameters", op
                        ));
                    }
                }
            }

            AstNode::MacroDefinitionNode(_) => {}
            AstNode::MacroInvocation { .. } => {}

            _ => {}
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Class definition
    // ─────────────────────────────────────────────────────────────────────────

    fn check_class_definition(
        &mut self,
        name:    &str,
        members: &[ClassMember],
        span:    Span,
    ) {
        let class_id = self.type_arena.alloc_class(ClassType {
            name:                 name.to_string(),
            fields:               vec![],
            field_index_map:      HashMap::new(),
            methods:              HashMap::new(),
            operators:            HashMap::new(),
            class_proto_constant: None,
        });

        self.scopes.define_type(name.to_string(), Type::Class(class_id));

        // ── Collect fields and method signatures ────────────────────────────
        let mut built_fields:    Vec<(String, Type, bool)>                              = Vec::new();
        let mut built_field_map: HashMap<String, usize>                                 = HashMap::new();
        let mut built_methods:   HashMap<String, (usize, usize, FunctionType, bool)>    = HashMap::new();

        for member in members {
            match member {
                ClassMember::Field { name, ty, is_public } => {
                    let compiled = self.compile_type(ty);
                    let idx      = built_fields.len();
                    built_field_map.insert(name.clone(), idx);
                    built_fields.push((name.clone(), compiled, *is_public));
                }
                ClassMember::Method { name, params, return_type, is_public, .. } => {
                    let fn_ty = self.build_fn_type(params, return_type);
                    built_methods.insert(name.clone(), (0, 0, fn_ty, *is_public));
                }
                _ => {}
            }
        }

        {
            let class             = self.type_arena.get_class_mut(class_id);
            class.fields          = built_fields;
            class.field_index_map = built_field_map;
            class.methods         = built_methods;
        }

        // ── Check method bodies ─────────────────────────────────────────────
        let prev_class     = self.current_class.replace(name.to_string());
        let prev_return_ty = self.current_return_ty.clone();

        self.scopes.push();
        self.scopes.define_local("self".into(), Type::Class(class_id), false);

        for member in members {
            match member {
                ClassMember::Method { name: method_name, params, return_type, body, .. } => {
                    // Clone everything needed from the arena before touching self.
                    let (fn_ty, _) = {
                        let class = self.type_arena.get_class(class_id);
                        let (_, _, ft, is_pub) = class.methods.get(method_name).unwrap();
                        (ft.clone(), *is_pub)
                    };

                    self.scopes.define_local(
                        method_name.clone(),
                        Type::Function(Box::new(fn_ty.clone())),
                        false,
                    );

                    self.current_return_ty = *fn_ty.return_type.clone();

                    self.scopes.push();
                    self.scopes.define_local("self".into(), Type::Class(class_id), false);
                    self.register_params(params);
                    for s in body { self.check_stmt(s); }
                    self.scopes.pop();
                }

                ClassMember::OperatorOverload { op, params, return_type, body } => {
                    if params.len() != 2 {
                        self.error(Span::dummy(), format!(
                            "operator '{:?}' overload must take exactly 2 parameters", op
                        ));
                    }

                    let ret_ty             = self.compile_type(return_type);
                    self.current_return_ty = ret_ty;

                    self.scopes.push();
                    self.register_params(params);
                    for s in body { self.check_stmt(s); }
                    self.scopes.pop();
                }

                _ => {}
            }
        }

        self.scopes.pop();

        self.current_class     = prev_class;
        self.current_return_ty = prev_return_ty;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Call checking
    // ─────────────────────────────────────────────────────────────────────────

    fn check_call(
        &mut self,
        callee: &SAst,
        args: &[SAst],
        span: Span,
    ) -> Type {
        let callee_ty = self.infer_expr_type(callee);

        match callee_ty {
            Type::Function(fn_ty) => {
                let params: Vec<Type> = fn_ty.params.clone();
                let ret_ty            = *fn_ty.return_type.clone();

                if args.len() != params.len() {
                    self.error(
                        span,
                        format!(
                            "expected {} argument(s), got {}",
                            params.len(),
                            args.len()
                        ),
                    );
                }

                for (arg, param_ty) in args.iter().zip(params.iter()) {
                    let arg_ty = self.infer_expr_type(arg);

                    match param_ty {
                        Type::Qualified { moved: true, .. } => {
                            if let AstNode::Identifier(n) = &arg.node {
                                if !matches!(arg_ty, Type::Qualified { moved: true, .. }) {
                                    self.error(
                                        arg.span,
                                        format!(
                                            "variable '{}' must be explicitly moved (use '->{}')",
                                            n,
                                            n
                                        ),
                                    );
                                }
                            }
                        }

                        Type::Qualified { borrowed: true, .. } => {
                            if let AstNode::Identifier(n) = &arg.node {
                                if !matches!(arg_ty, Type::Qualified { borrowed: true, .. }) {
                                    self.error(
                                        arg.span,
                                        format!(
                                            "variable '{}' must be explicitly borrowed (use '&{}')",
                                            n,
                                            n
                                        ),
                                    );
                                }
                            }
                        }

                        _ => {
                            self.assert_assignable(param_ty, &arg_ty, arg.span);
                        }
                    }
                }

                ret_ty
            }

            Type::Unknown => Type::Unknown,

            _ => {
                self.error(span, "attempt to call non-function");
                Type::Unknown
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Expression type inference
    // ─────────────────────────────────────────────────────────────────────────

    pub fn infer_expr_type(&mut self, expr: &SAst) -> Type {
        match &expr.node {
            AstNode::IntLiteral(_)    => Type::I32,
            AstNode::FloatLiteral(_)  => Type::F64,
            AstNode::StringLiteral(_) => Type::String,

            AstNode::Identifier(name) => {
                // Clone out of scope before any mutable borrow.
                let info = self.scopes.resolve_local(name)
                    .map(|l| (l.moved, l.ty.clone()));

                match info {
                    Some((moved, ty)) => {
                        if moved {
                            self.error(expr.span, format!("use of moved variable '{}'", name));
                        }
                        ty
                    }
                    None => {
                        // Try type namespace.
                        let ty_opt = self.scopes.resolve_type(name).cloned();
                        match ty_opt {
                            Some(ty) => ty,
                            None => {
                                self.error(expr.span, format!("undefined variable '{}'", name));
                                Type::Unknown
                            }
                        }
                    }
                }
            }

            AstNode::SelfExpr => {
                let ty_opt = self.current_class.as_ref()
                    .and_then(|n| self.scopes.resolve_type(n))
                    .cloned();
                ty_opt.unwrap_or(Type::Unknown)
            }

            AstNode::Borrowed(inner) => {
                let inner_ty = self.infer_expr_type(inner);
                Type::Qualified {
                    inner:    Box::new(inner_ty),
                    mutable:  false,
                    borrowed: true,
                    moved:    false,
                }
            }

            AstNode::Moved(inner) => {
                match &inner.node {
                    AstNode::Identifier(name) => {
                        let name = name.clone();
                        // Clone before any mutable borrow.
                        let info = self.scopes.resolve_local(&name)
                            .map(|l| (l.moved, l.ty.clone()));

                        match info {
                            Some((already_moved, ty)) => {
                                if already_moved {
                                    self.error(expr.span, format!(
                                        "use of already-moved variable '{}'", name
                                    ));
                                    return Type::Unknown;
                                }
                                self.scopes.mark_moved(&name);
                                Type::Qualified {
                                    inner:    Box::new(ty),
                                    mutable:  false,
                                    borrowed: false,
                                    moved:    true,
                                }
                            }
                            None => {
                                self.error(expr.span, format!("undefined variable '{}'", name));
                                Type::Unknown
                            }
                        }
                    }
                    _ => {
                        let inner_ty = self.infer_expr_type(inner);
                        Type::Qualified {
                            inner:    Box::new(inner_ty),
                            mutable:  false,
                            borrowed: false,
                            moved:    true,
                        }
                    }
                }
            }

            AstNode::BinaryOperation { left, right, op } => {
                let lt = self.infer_expr_type(left);
                let rt = self.infer_expr_type(right);

                // Clone overload out of arena before any further mutable borrows.
                let overload = match (lt.inner(), rt.inner()) {
                    (Type::Class(id), _) | (_, Type::Class(id)) => {
                        self.type_arena.get_class(*id).operators.get(op).cloned()
                    }
                    _ => None,
                };

                if let Some((_, fn_ty)) = overload {
                    if fn_ty.params.len() == 2 {
                        let p0     = fn_ty.params[0].clone();
                        let p1     = fn_ty.params[1].clone();
                        let ret_ty = *fn_ty.return_type.clone();
                        self.assert_assignable(&p0, lt.inner(), left.span);
                        self.assert_assignable(&p1, rt.inner(), right.span);
                        return ret_ty;
                    } else {
                        self.error(expr.span, format!(
                            "operator '{:?}' overload must take exactly 2 parameters", op
                        ));
                    }
                }

                if matches!(lt, Type::F64) || matches!(rt, Type::F64) { return Type::F64; }
                if matches!(lt, Type::F32) || matches!(rt, Type::F32) { return Type::F32; }
                if lt != Type::Unknown { lt } else { rt }
            }
            
            AstNode::FunctionCall { callee, args } => {
                self.check_call(callee, args, expr.span)
            }

            AstNode::DotIndex { indexee, index } => {
                // Try namespace path first (returns a borrow, so clone it).
                let ns_result = self.resolve_namespace_path(&expr.node).cloned();
                if let Some(ns) = ns_result {
                    let field = match &index.node {
                        AstNode::Identifier(s) => s.as_str(),
                        _ => return Type::Unknown,
                    };
                    if let Some(ty) = ns.locals.get(field) { return ty.clone(); }
                    if let Some(ty) = ns.types.get(field)  { return ty.clone(); }
                }

                let obj_ty     = self.infer_expr_type(indexee);
                let field_name = match &index.node {
                    AstNode::Identifier(s) => s.clone(),
                    _ => return Type::Unknown,
                };

                match obj_ty {
                    Type::Class(id) => {
                        // Clone everything we need before calling self.error.
                        let class      = self.type_arena.get_class(id);
                        let owner_name = class.name.clone();

                        let field_hit = class.fields.iter()
                            .find(|(n, _, _)| n == &field_name)
                            .map(|(_, ty, is_pub)| (ty.clone(), *is_pub));

                        let method_hit = class.methods.get(&field_name)
                            .map(|(_, _, fn_ty, is_pub)| (fn_ty.clone(), *is_pub));

                        let current_class = self.current_class.clone();

                        if let Some((ty, is_pub)) = field_hit {
                            if !is_pub && current_class.as_deref() != Some(&owner_name) {
                                self.error(expr.span, format!(
                                    "field '{}' of '{}' is private", field_name, owner_name
                                ));
                            }
                            return ty;
                        }

                        if let Some((fn_ty, is_pub)) = method_hit {
                            if !is_pub && current_class.as_deref() != Some(&owner_name) {
                                self.error(expr.span, format!(
                                    "method '{}' of '{}' is private", field_name, owner_name
                                ));
                            }
                            return Type::Function(Box::new(fn_ty));
                        }

                        self.error(expr.span, format!("type has no member '{}'", field_name));
                        Type::Unknown
                    }
                    Type::Unknown => Type::Unknown,
                    _ => {
                        self.error(expr.span, "attempt to index non-class value");
                        Type::Unknown
                    }
                }
            }

            AstNode::MethodCall { indexee, index } => {
                let obj_ty      = self.infer_expr_type(indexee);
                let method_name = match &index.node {
                    AstNode::Identifier(s) => s.clone(),
                    _ => return Type::Unknown,
                };

                match obj_ty {
                    Type::Class(id) => {
                        // Clone out of arena before calling self.error.
                        let class      = self.type_arena.get_class(id);
                        let owner_name = class.name.clone();
                        let hit        = class.methods.get(&method_name)
                            .map(|(_, _, fn_ty, is_pub)| (fn_ty.clone(), *is_pub));
                        
                        let current_class = self.current_class.clone();

                        match hit {
                            Some((fn_ty, is_pub)) => {
                                if !is_pub && current_class.as_deref() != Some(&owner_name) {
                                    self.error(expr.span, format!(
                                        "method '{}' of '{}' is private", method_name, owner_name
                                    ));
                                }
                                Type::Function(Box::new(fn_ty))
                            }
                            None => {
                                self.error(expr.span, format!("unknown method '{}'", method_name));
                                Type::Unknown
                            }
                        }
                    }
                    Type::Unknown => Type::Unknown,
                    _ => {
                        self.error(expr.span, "attempt to call method on non-class");
                        Type::Unknown
                    }
                }
            }

            AstNode::ClassLiteral { ty, fields } => {
                let class_name = match &ty.node {
                    AstNode::Identifier(s) => s.clone(),
                    AstNode::SelfExpr      => self.current_class.clone().unwrap_or_default(),
                    _ => return Type::Unknown,
                };

                let class_ty = match self.scopes.resolve_type(&class_name).cloned() {
                    Some(t) => t,
                    None => {
                        self.error(expr.span, format!("unknown type '{}'", class_name));
                        return Type::Unknown;
                    }
                };

                if let Type::Class(id) = &class_ty {
                    let id = *id;
                    for (fname, fexpr) in fields {
                        // Infer the actual type first (may mutably borrow self).
                        let actual_ty = self.infer_expr_type(fexpr);

                        // Now clone field info out of the arena.
                        let class     = self.type_arena.get_class(id);
                        let field_hit = class.fields.iter()
                            .find(|(n, _, _)| n == fname)
                            .map(|(_, ty, _)| ty.clone());
                        
                        match field_hit {
                            Some(expected_ty) => {
                                self.assert_assignable(&expected_ty, &actual_ty, fexpr.span);
                            }
                            None => {
                                self.error(fexpr.span, format!(
                                    "class '{}' has no field '{}'", class_name, fname
                                ));
                            }
                        }
                    }
                }

                class_ty
            }

            AstNode::TypeCast { left, right } => {
                self.infer_expr_type(left);
                self.compile_type(right)
            }

            AstNode::Typeof(inner) => {
                self.infer_expr_type(inner);
                Type::TypeVar("Type".to_string())
            }

            _ => Type::Unknown,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn build_fn_type(&self, params: &[BindingNode], return_type: &TypeNode) -> FunctionType {
        FunctionType {
            params: params.iter().map(|p| match p {
                BindingNode::IdentifierBinding { ty, .. } => self.compile_type(ty),
                _ => Type::Unknown,
            }).collect(),
            return_type: Box::new(self.compile_type(return_type)),
        }
    }

    fn register_params(&mut self, params: &[BindingNode]) {
        // Compile all types first (immutable borrows only), then define.
        let compiled: Vec<(String, Type)> = params.iter().filter_map(|p| {
            if let BindingNode::IdentifierBinding { name, ty } = p {
                Some((name.clone(), self.compile_type(ty)))
            } else {
                None
            }
        }).collect();

        for (name, ty) in compiled {
            if matches!(ty, Type::Unknown) && name != "self" {
                self.errors.push(TypeError {
                    span:    Span::dummy(),
                    message: format!("parameter '{}' must have an explicit type", name),
                });
            }
            self.scopes.define_local(name, ty, false);
        }
    }

    pub fn compile_type(&self, node: &TypeNode) -> Type {
        match node {
            TypeNode::Inferred => Type::Unknown,
            TypeNode::ArrayType { elem_type } => {
                Type::Array(Box::new(self.compile_type(elem_type)))
            }
            TypeNode::Qualified { inner, mutable, borrowed, moved } => {
                Type::Qualified {
                    inner:    Box::new(self.compile_type(inner)),
                    mutable:  *mutable,
                    borrowed: *borrowed,
                    moved:    *moved,
                }
            }
            TypeNode::NominalType { name, generics } => {
                if let Some(builtin) = Self::resolve_builtin(name) { return builtin; }
                if let Some(ty) = self.scopes.resolve_type(name) { return ty.clone(); }
                let args: Vec<Type> = generics.iter().map(|g| self.compile_type(g)).collect();
                if args.is_empty() { Type::TypeVar(name.clone()) }
                else { Type::Generic { name: name.clone(), args } }
            }
            _ => Type::Unknown,
        }
    }

    pub fn resolve_builtin(name: &str) -> Option<Type> {
        match name {
            "u8"     => Some(Type::U8),    "i8"     => Some(Type::I8),
            "u16"    => Some(Type::U16),   "i16"    => Some(Type::I16),
            "u32"    => Some(Type::U32),   "i32"    => Some(Type::I32),
            "u64"    => Some(Type::U64),   "i64"    => Some(Type::I64),
            "usize"  => Some(Type::USize), "bool"   => Some(Type::Bool),
            "string" => Some(Type::String),"empty"  => Some(Type::Empty),
            _        => None,
        }
    }

    pub fn assert_assignable(&mut self, lhs: &Type, rhs: &Type, span: Span) {
        if matches!(lhs, Type::Unknown) || matches!(rhs, Type::Unknown) { return; }

        // Clone to avoid holding borrows into self while calling self.error.
        let lhs = lhs.clone();
        let rhs = rhs.clone();

        match (&lhs, &rhs) {
            (
                Type::Qualified { borrowed: true, inner: li, .. },
                Type::Qualified { borrowed: true, inner: ri, .. },
            ) => {
                let li = li.as_ref().clone();
                let ri = ri.as_ref().clone();
                self.assert_assignable(&li, &ri, span);
            }
            (Type::Qualified { borrowed: true, .. }, _) => {
                self.error(span, format!("expected borrowed '{}', got '{}'", lhs, rhs));
            }
            (
                Type::Qualified { moved: true, inner: li, .. },
                Type::Qualified { moved: true, inner: ri, .. },
            ) => {
                let li = li.as_ref().clone();
                let ri = ri.as_ref().clone();
                self.assert_assignable(&li, &ri, span);
            }
            (Type::Qualified { moved: true, .. }, _) => {
                self.error(span, format!("expected moved '{}', got '{}'", lhs, rhs));
            }
            _ => {
                if lhs != *rhs.inner() && *lhs.inner() != *rhs.inner() {
                    self.error(span, format!("expected '{}', got '{}'", lhs, rhs));
                }
            }
        }
    }

    pub fn resolve_namespace_path(&self, path: &AstNode) -> Option<&Namespace> {
        let mut parts = Vec::<&str>::new();

        fn collect_parts<'a>(node: &'a AstNode, out: &mut Vec<&'a str>) -> bool {
            match node {
                AstNode::Identifier(name) => { out.push(name); true }
                AstNode::DotIndex { indexee, index } => {
                    if !collect_parts(&indexee.node, out) { return false; }
                    match &index.node {
                        AstNode::Identifier(name) => { out.push(name); true }
                        _ => false,
                    }
                }
                _ => false,
            }
        }

        if !collect_parts(path, &mut parts) { return None; }

        let mut current: Option<&Namespace> = None;
        for (i, part) in parts.iter().enumerate() {
            if i == 0 {
                current = self.scopes.resolve_namespace(part);
            } else {
                current = current?.children.get(*part);
            }
        }
        current
    }
}