use std::rc::Rc;

use crate::ast::{
    AssignTarget, BinaryOp, Expr, FnParam, ImportSource, Pattern, Program, Stmt, UnaryOp,
};
use crate::bytecode::{Chunk, Constant, FunctionProto, Op, StructProto, UpvalueDesc};
use crate::lexer::Span;

struct Local {
    name: String,
    depth: usize,
    slot: u16,
    is_captured: bool,
    is_pub: bool,
}

struct LoopContext {
    break_stack_height: usize,
    continue_stack_height: usize,
    try_depth: usize,
    scope_depth: usize,
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

struct FunctionCompiler {
    name: String,
    arity: u8,
    is_main: bool,
    chunk: Chunk,
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDesc>,
    scope_depth: usize,
    temp_depth: usize,
    try_depth: usize,
    cleanup_scopes: Vec<usize>,
    loops: Vec<LoopContext>,
}

impl FunctionCompiler {
    fn new(name: String, arity: u8, is_main: bool) -> Self {
        let locals = vec![Local {
            name: String::new(),
            depth: 0,
            slot: 0,
            is_captured: false,
            is_pub: false,
        }];
        Self {
            name,
            arity,
            is_main,
            chunk: Chunk::default(),
            locals,
            upvalues: Vec::new(),
            scope_depth: 0,
            temp_depth: 0,
            try_depth: 0,
            cleanup_scopes: Vec::new(),
            loops: Vec::new(),
        }
    }

    fn stack_height(&self) -> usize {
        self.locals.len() + self.temp_depth
    }
}

pub struct Compiler {
    frames: Vec<FunctionCompiler>,
    include_specs: bool,
}

impl Compiler {
    fn local_count(chunk: &Chunk, minimum: u16) -> u16 {
        let mut max = minimum.saturating_sub(1);
        for op in &chunk.code {
            let slot = match op {
                crate::bytecode::Op::GetLocal(i)
                | crate::bytecode::Op::SetLocal(i)
                | crate::bytecode::Op::AddLocalInt(i, _)
                | crate::bytecode::Op::SubLocalInt(i, _)
                | crate::bytecode::Op::LtLocalInt(i, _)
                | crate::bytecode::Op::LeLocalInt(i, _)
                | crate::bytecode::Op::IncLocalPop(i, _) => Some(*i),
                crate::bytecode::Op::AddLocals(a, b)
                | crate::bytecode::Op::SubLocals(a, b)
                | crate::bytecode::Op::LtLocals(a, b) => Some((*a).max(*b)),
                crate::bytecode::Op::ForPrep(i, _) | crate::bytecode::Op::ForIter(i, _) => {
                    Some(i.saturating_add(2))
                }
                _ => None,
            };
            if let Some(slot) = slot {
                max = max.max(slot);
            }
        }
        for list in &chunk.upvalue_lists {
            for uv in list {
                if uv.is_local {
                    max = max.max(uv.index);
                }
            }
        }
        max.saturating_add(1)
    }
    pub fn compile(program: &Program) -> Result<Rc<FunctionProto>, String> {
        Self::compile_with_mode(program, false)
    }

    pub fn compile_with_specs(program: &Program) -> Result<Rc<FunctionProto>, String> {
        Self::compile_with_mode(program, true)
    }

    fn compile_with_mode(
        program: &Program,
        include_specs: bool,
    ) -> Result<Rc<FunctionProto>, String> {
        let mut compiler = Self {
            frames: vec![FunctionCompiler::new("<script>".to_string(), 0, true)],
            include_specs,
        };

        for stmt in &program.stmts {
            compiler.compile_stmt(stmt)?;
        }

        if let Some(tail) = &program.tail {
            compiler.compile_expr(tail)?;
            let line = tail.span().line;
            if !compiler.current().cleanup_scopes.is_empty() {
                compiler.emit(Op::RunCleanups(0), line);
            }
            compiler.emit(Op::Return, line);
        } else {
            compiler.emit(Op::Unit, 0);
            if !compiler.current().cleanup_scopes.is_empty() {
                compiler.emit(Op::RunCleanups(0), 0);
            }
            compiler.emit(Op::Return, 0);
        }

        let frame = compiler.frames.pop().unwrap();
        if frame.chunk.limit_exceeded {
            return Err("Bytecode constant/local limit exceeded".to_string());
        }
        Ok(Rc::new(FunctionProto {
            ic_id: crate::bytecode::next_ic_id(),
            name: frame.name,
            arity: frame.arity,
            min_arity: 0,
            is_variadic: false,
            param_names: Vec::new(),
            local_count: Self::local_count(&frame.chunk, frame.arity as u16 + 1),
            upvalue_count: frame.upvalues.len() as u16,
            is_main: frame.is_main,
            chunk: frame.chunk,
        }))
    }

    pub fn compile_module(
        program: &Program,
        module_name: &str,
    ) -> Result<Rc<FunctionProto>, String> {
        let mut compiler = Self {
            frames: vec![FunctionCompiler::new(
                format!("<module {}>", module_name),
                0,
                false,
            )],
            include_specs: false,
        };
        compiler.current().scope_depth = 1;

        for stmt in &program.stmts {
            compiler.compile_stmt(stmt)?;
        }

        if let Some(tail) = &program.tail {
            compiler.compile_expr(tail)?;
            compiler.emit(Op::Pop, tail.span().line);
        }

        let has_any_pub = compiler
            .current()
            .locals
            .iter()
            .skip(1)
            .any(|l| l.depth == 1 && l.is_pub);
        let exports: Vec<(String, u16)> = compiler
            .current()
            .locals
            .iter()
            .skip(1)
            .filter(|l| l.depth == 1 && !l.name.is_empty() && (!has_any_pub || l.is_pub))
            .map(|l| (l.name.clone(), l.slot))
            .collect();

        let count = exports.len() as u16;
        for (name, slot) in exports {
            let name_idx = compiler.current().chunk.add_string_constant(&name);
            compiler.emit(Op::Constant(name_idx), 0);
            compiler.emit(Op::GetLocal(slot), 0);
        }

        let mod_name_idx = compiler.current().chunk.add_string_constant(module_name);
        compiler.emit(Op::MakeModule(mod_name_idx, count), 0);
        if !compiler.current().cleanup_scopes.is_empty() {
            compiler.emit(Op::RunCleanups(0), 0);
        }
        compiler.emit(Op::Return, 0);

        let frame = compiler.frames.pop().unwrap();
        if frame.chunk.limit_exceeded {
            return Err("Bytecode constant/local limit exceeded".to_string());
        }
        Ok(Rc::new(FunctionProto {
            ic_id: crate::bytecode::next_ic_id(),
            name: frame.name,
            arity: frame.arity,
            min_arity: 0,
            is_variadic: false,
            param_names: Vec::new(),
            local_count: Self::local_count(&frame.chunk, frame.arity as u16 + 1),
            upvalue_count: frame.upvalues.len() as u16,
            is_main: frame.is_main,
            chunk: frame.chunk,
        }))
    }

    fn current(&mut self) -> &mut FunctionCompiler {
        self.frames.last_mut().unwrap()
    }

    fn emit(&mut self, op: Op, line: usize) -> usize {
        self.current().chunk.emit(op, line)
    }

    fn current_ip(&self) -> usize {
        self.frames.last().unwrap().chunk.code.len()
    }

    fn patch_jump(&mut self, op_idx: usize) {
        let target = self.current_ip();
        self.current().chunk.patch_jump(op_idx, target);
    }

    fn begin_scope(&mut self) {
        self.current().scope_depth += 1;
    }

    fn end_scope(&mut self, line: usize) {
        let target_depth = self.current().scope_depth - 1;
        self.current().scope_depth = target_depth;
        if self
            .current()
            .cleanup_scopes
            .iter()
            .any(|&d| d > target_depth)
        {
            self.emit(Op::RunCleanups((target_depth + 1) as u16), line);
            self.current().cleanup_scopes.retain(|&d| d <= target_depth);
        }

        while let Some(local) = self.current().locals.last() {
            if local.depth <= target_depth {
                break;
            }
            let captured = local.is_captured;
            self.current().locals.pop();
            if captured {
                self.emit(Op::CloseUpvalue, line);
            } else {
                self.emit(Op::Pop, line);
            }
        }
    }

    fn push_temps(&mut self, count: usize) {
        self.current().temp_depth += count;
    }

    fn pop_temps(&mut self, count: usize) {
        self.current().temp_depth -= count;
    }

    fn add_local(&mut self, name: String) -> u16 {
        self.add_local_with_pub(name, false)
    }

    fn add_local_with_pub(&mut self, name: String, is_pub: bool) -> u16 {
        let stack_idx = self.current().stack_height();
        if stack_idx > u16::MAX as usize {
            self.current().chunk.limit_exceeded = true;
            return 0;
        }
        let depth = self.current().scope_depth;
        let slot = stack_idx as u16;
        self.current().locals.push(Local {
            name,
            depth,
            slot,
            is_captured: false,
            is_pub,
        });
        slot
    }

    fn check_pub(&self, is_pub: bool, span: Span) -> Result<(), String> {
        if is_pub && (self.frames.len() > 1 || self.frames.last().unwrap().scope_depth > 1) {
            return Err(format!(
                "[{}:{}] 'pub' is only allowed on top-level declarations",
                span.line, span.col
            ));
        }
        Ok(())
    }

    fn resolve_local(frame: &FunctionCompiler, name: &str) -> Option<(usize, u16)> {
        for (i, local) in frame.locals.iter().enumerate().rev() {
            if local.name == name {
                return Some((i, local.slot));
            }
        }
        None
    }

    fn resolve_upvalue(&mut self, frame_idx: usize, name: &str) -> Option<u16> {
        if frame_idx == 0 {
            return None;
        }
        let parent_idx = frame_idx - 1;
        if let Some((local_idx, slot)) = Self::resolve_local(&self.frames[parent_idx], name) {
            self.frames[parent_idx].locals[local_idx].is_captured = true;
            return Some(self.add_upvalue(frame_idx, slot, true));
        }
        if let Some(upval_idx) = self.resolve_upvalue(parent_idx, name) {
            return Some(self.add_upvalue(frame_idx, upval_idx, false));
        }
        None
    }

    fn add_upvalue(&mut self, frame_idx: usize, index: u16, is_local: bool) -> u16 {
        let upvalues = &mut self.frames[frame_idx].upvalues;
        for (i, uv) in upvalues.iter().enumerate() {
            if uv.index == index && uv.is_local == is_local {
                return i as u16;
            }
        }
        upvalues.push(UpvalueDesc { index, is_local });
        (upvalues.len() - 1) as u16
    }

    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Import {
                source,
                alias,
                span,
            } => {
                let (raw, is_path) = match source {
                    ImportSource::Name(n) => (n.as_str(), false),
                    ImportSource::Path(p) => (p.as_str(), true),
                };
                let src_idx = self.current().chunk.add_string_constant(raw);
                self.emit(Op::Import(src_idx, is_path), span.line);
                if self.current().scope_depth == 0 {
                    let alias_idx = self.current().chunk.add_string_constant(alias);
                    self.emit(Op::DefineGlobal(alias_idx), span.line);
                } else {
                    self.add_local(alias.clone());
                }
                Ok(())
            }
            Stmt::ImportFrom {
                source,
                items,
                span,
            } => {
                let (raw, is_path) = match source {
                    ImportSource::Name(n) => (n.as_str(), false),
                    ImportSource::Path(p) => (p.as_str(), true),
                };
                let src_idx = self.current().chunk.add_string_constant(raw);
                let top_level = self.current().scope_depth == 0;
                self.emit(Op::Import(src_idx, is_path), span.line);
                if top_level {
                    self.begin_scope();
                }
                let mod_slot = self.add_local(String::new());
                for item in items {
                    self.emit(Op::GetLocal(mod_slot), span.line);
                    let name_idx = self.current().chunk.add_string_constant(&item.name);
                    self.emit(Op::GetAttr(name_idx), span.line);
                    if top_level {
                        let alias_idx = self.current().chunk.add_string_constant(&item.alias);
                        self.emit(Op::DefineGlobal(alias_idx), span.line);
                    } else {
                        self.add_local(item.alias.clone());
                    }
                }
                if top_level {
                    self.end_scope(span.line);
                }
                Ok(())
            }
            Stmt::Let {
                name,
                value,
                is_pub,
                span,
            } => {
                self.check_pub(*is_pub, *span)?;
                self.compile_expr(value)?;
                if self.current().scope_depth == 0 {
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::DefineGlobal(idx), span.line);
                } else {
                    self.add_local_with_pub(name.clone(), *is_pub);
                }
                Ok(())
            }
            Stmt::LetList {
                names,
                value,
                is_pub,
                span,
            } => {
                self.check_pub(*is_pub, *span)?;
                let top_level = self.current().scope_depth == 0;
                self.compile_expr(value)?;
                if top_level {
                    self.begin_scope();
                }
                let tmp = self.add_local(String::new());
                for (i, name) in names.iter().enumerate() {
                    self.emit(Op::GetLocal(tmp), span.line);
                    let idx_c = self.current().chunk.add_constant(Constant::Int(i as i64));
                    self.emit(Op::Constant(idx_c), span.line);
                    self.emit(Op::GetIndex, span.line);
                    if top_level {
                        let idx = self.current().chunk.add_string_constant(name);
                        self.emit(Op::DefineGlobal(idx), span.line);
                    } else {
                        self.add_local_with_pub(name.clone(), *is_pub);
                    }
                }
                if top_level {
                    self.end_scope(span.line);
                }
                Ok(())
            }
            Stmt::LetDict {
                fields,
                value,
                is_pub,
                span,
            } => {
                self.check_pub(*is_pub, *span)?;
                let top_level = self.current().scope_depth == 0;
                self.compile_expr(value)?;
                if top_level {
                    self.begin_scope();
                }
                let tmp = self.add_local(String::new());
                for field in fields {
                    self.emit(Op::GetLocal(tmp), span.line);
                    let f_idx = self.current().chunk.add_string_constant(field);
                    self.emit(Op::GetAttr(f_idx), span.line);
                    if top_level {
                        let idx = self.current().chunk.add_string_constant(field);
                        self.emit(Op::DefineGlobal(idx), span.line);
                    } else {
                        self.add_local_with_pub(field.clone(), *is_pub);
                    }
                }
                if top_level {
                    self.end_scope(span.line);
                }
                Ok(())
            }
            Stmt::FnDecl {
                name,
                params,
                body,
                is_pub,
                span,
            } => {
                self.check_pub(*is_pub, *span)?;
                if self.current().scope_depth == 0 {
                    self.compile_function(name.clone(), params, body, *span)?;
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::DefineGlobal(idx), span.line);
                } else {
                    self.emit(Op::Unit, span.line);
                    let slot = self.add_local_with_pub(name.clone(), *is_pub);
                    self.compile_function(name.clone(), params, body, *span)?;
                    self.emit(Op::SetLocal(slot), span.line);
                    self.emit(Op::Pop, span.line);
                }
                Ok(())
            }
            Stmt::StructDecl {
                name,
                is_pub,
                fields,
                methods,
                span,
            } => {
                self.check_pub(*is_pub, *span)?;
                let local_slot = if self.current().scope_depth > 0 {
                    self.emit(Op::Unit, span.line);
                    Some(self.add_local_with_pub(name.clone(), *is_pub))
                } else {
                    None
                };
                let struct_proto = Rc::new(StructProto {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .map(|f| (f.name.clone(), f.default.is_some()))
                        .collect(),
                    methods: methods.iter().map(|m| m.name.clone()).collect(),
                });
                let proto_idx = self
                    .current()
                    .chunk
                    .add_constant(Constant::Struct(struct_proto));

                let mut pushed = 0usize;
                for field in fields {
                    if let Some(default_expr) = &field.default {
                        self.compile_function(
                            format!("{}::default_{}", name, field.name),
                            &[],
                            default_expr,
                            default_expr.span(),
                        )?;
                        self.push_temps(1);
                        pushed += 1;
                    }
                }
                for method in methods {
                    self.compile_function(
                        format!("{}.{}", name, method.name),
                        &method.params,
                        &method.body,
                        method.span,
                    )?;
                    self.push_temps(1);
                    pushed += 1;
                }
                self.pop_temps(pushed);
                self.emit(Op::MakeStructDef(proto_idx), span.line);
                if let Some(slot) = local_slot {
                    self.emit(Op::SetLocal(slot), span.line);
                    self.emit(Op::Pop, span.line);
                } else {
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::DefineGlobal(idx), span.line);
                }
                Ok(())
            }
            Stmt::While { cond, body, span } => {
                let loop_start = self.current_ip();
                let stack_height = self.current().stack_height();
                let loop_try_depth = self.current().try_depth;
                let loop_scope_depth = self.current().scope_depth;
                self.current().loops.push(LoopContext {
                    break_stack_height: stack_height,
                    continue_stack_height: stack_height,
                    try_depth: loop_try_depth,
                    scope_depth: loop_scope_depth,
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });

                self.compile_expr(cond)?;
                let exit_jump = self.emit(Op::JumpIfFalsePop(0), span.line);

                self.compile_expr(body)?;
                self.emit(Op::Pop, span.line);

                let ctx = self.current().loops.pop().unwrap();
                for j in ctx.continue_jumps {
                    self.current().chunk.patch_jump(j, loop_start);
                }

                self.emit(Op::Jump(loop_start), span.line);
                self.patch_jump(exit_jump);

                for j in ctx.break_jumps {
                    self.patch_jump(j);
                }
                Ok(())
            }
            Stmt::For {
                var,
                iter,
                body,
                span,
            } => {
                let break_stack_height = self.current().stack_height();
                self.begin_scope();
                self.compile_expr(iter)?;
                let iter_slot = self.add_local("$for_iter".to_string());
                self.emit(Op::Unit, span.line);
                let _idx_slot = self.add_local("$for_idx".to_string());
                self.emit(Op::Unit, span.line);
                let _end_slot = self.add_local("$for_end".to_string());

                let prep_jump = self.emit(Op::ForPrep(iter_slot, 0), span.line);
                let loop_start = self.current_ip();
                let exit_jump = self.emit(Op::ForIter(iter_slot, 0), span.line);

                let continue_stack_height = self.current().stack_height();
                let loop_try_depth = self.current().try_depth;
                let loop_scope_depth = self.current().scope_depth;
                self.current().loops.push(LoopContext {
                    break_stack_height,
                    continue_stack_height,
                    try_depth: loop_try_depth,
                    scope_depth: loop_scope_depth,
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });

                self.begin_scope();
                self.add_local(var.clone());

                self.compile_expr(body)?;
                self.emit(Op::Pop, span.line);
                self.end_scope(span.line);

                let continue_target = self.current_ip();
                let ctx = self.current().loops.pop().unwrap();
                for j in ctx.continue_jumps {
                    self.current().chunk.patch_jump(j, continue_target);
                }

                self.emit(Op::Jump(loop_start), span.line);
                self.patch_jump(prep_jump);
                self.patch_jump(exit_jump);

                self.end_scope(span.line);

                for j in ctx.break_jumps {
                    self.patch_jump(j);
                }
                Ok(())
            }
            Stmt::ForPair {
                var1,
                var2,
                iter,
                body,
                span,
            } => {
                let break_stack_height = self.current().stack_height();
                self.begin_scope();
                let pairs_fn = self.current().chunk.add_string_constant("__pairs");
                self.emit(Op::GetGlobal(pairs_fn), span.line);
                self.push_temps(1);
                self.compile_expr(iter)?;
                self.pop_temps(1);
                self.emit(Op::Call(1), span.line);
                let iter_slot = self.add_local("$for_iter".to_string());

                let idx_const = self.current().chunk.add_constant(Constant::Int(0));
                self.emit(Op::Constant(idx_const), span.line);
                let idx_slot = self.add_local("$for_idx".to_string());

                let len_name = self.current().chunk.add_string_constant("len");
                self.emit(Op::GetGlobal(len_name), span.line);
                self.emit(Op::GetLocal(iter_slot), span.line);
                self.emit(Op::Call(1), span.line);
                let len_slot = self.add_local("$for_len".to_string());

                let loop_start = self.current_ip();
                self.emit(Op::GetLocal(idx_slot), span.line);
                self.emit(Op::GetLocal(len_slot), span.line);
                self.emit(Op::Lt, span.line);
                let exit_jump = self.emit(Op::JumpIfFalsePop(0), span.line);

                let continue_stack_height = self.current().stack_height();
                let loop_try_depth = self.current().try_depth;
                let loop_scope_depth = self.current().scope_depth;
                self.current().loops.push(LoopContext {
                    break_stack_height,
                    continue_stack_height,
                    try_depth: loop_try_depth,
                    scope_depth: loop_scope_depth,
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });

                self.begin_scope();
                self.emit(Op::GetLocal(iter_slot), span.line);
                self.emit(Op::GetLocal(idx_slot), span.line);
                self.emit(Op::GetIndex, span.line);
                let pair_slot = self.add_local(String::new());

                self.emit(Op::GetLocal(pair_slot), span.line);
                let c0 = self.current().chunk.add_constant(Constant::Int(0));
                self.emit(Op::Constant(c0), span.line);
                self.emit(Op::GetIndex, span.line);
                self.add_local(var1.clone());

                self.emit(Op::GetLocal(pair_slot), span.line);
                let c1 = self.current().chunk.add_constant(Constant::Int(1));
                self.emit(Op::Constant(c1), span.line);
                self.emit(Op::GetIndex, span.line);
                self.add_local(var2.clone());

                self.compile_expr(body)?;
                self.emit(Op::Pop, span.line);
                self.end_scope(span.line);

                let continue_target = self.current_ip();
                let ctx = self.current().loops.pop().unwrap();
                for j in ctx.continue_jumps {
                    self.current().chunk.patch_jump(j, continue_target);
                }

                let one_const = self.current().chunk.add_constant(Constant::Int(1));
                self.emit(Op::GetLocal(idx_slot), span.line);
                self.emit(Op::Constant(one_const), span.line);
                self.emit(Op::Add, span.line);
                self.emit(Op::SetLocal(idx_slot), span.line);
                self.emit(Op::Pop, span.line);

                self.emit(Op::Jump(loop_start), span.line);
                self.patch_jump(exit_jump);

                self.end_scope(span.line);

                for j in ctx.break_jumps {
                    self.patch_jump(j);
                }
                Ok(())
            }
            Stmt::Return { value, span } => {
                if self.frames.len() <= 1 {
                    return Err(format!(
                        "[{}:{}] Cannot 'return' from top-level script",
                        span.line, span.col
                    ));
                }
                if let Some(expr) = value {
                    let in_tail =
                        self.current().try_depth == 0 && self.current().cleanup_scopes.is_empty();
                    self.compile_expr_inner(expr, in_tail)?;
                } else {
                    self.emit(Op::Unit, span.line);
                }
                if !self.current().cleanup_scopes.is_empty() {
                    self.emit(Op::RunCleanups(0), span.line);
                }
                self.emit(Op::Return, span.line);
                Ok(())
            }
            Stmt::Break(span) => {
                if self.current().loops.is_empty() {
                    return Err(format!(
                        "[{}:{}] 'break' used outside of a loop",
                        span.line, span.col
                    ));
                }
                let (target_height, loop_try_depth, loop_scope_depth) = {
                    let l = self.current().loops.last().unwrap();
                    (l.break_stack_height, l.try_depth, l.scope_depth)
                };
                if self
                    .current()
                    .cleanup_scopes
                    .iter()
                    .any(|&d| d > loop_scope_depth)
                {
                    self.emit(Op::RunCleanups((loop_scope_depth + 1) as u16), span.line);
                }
                for _ in loop_try_depth..self.current().try_depth {
                    self.emit(Op::PopTry, span.line);
                }
                self.pop_stack_to_height(target_height, span.line);
                let j = self.emit(Op::Jump(0), span.line);
                self.current().loops.last_mut().unwrap().break_jumps.push(j);
                Ok(())
            }
            Stmt::Continue(span) => {
                if self.current().loops.is_empty() {
                    return Err(format!(
                        "[{}:{}] 'continue' used outside of a loop",
                        span.line, span.col
                    ));
                }
                let (target_height, loop_try_depth, loop_scope_depth) = {
                    let l = self.current().loops.last().unwrap();
                    (l.continue_stack_height, l.try_depth, l.scope_depth)
                };
                if self
                    .current()
                    .cleanup_scopes
                    .iter()
                    .any(|&d| d > loop_scope_depth)
                {
                    self.emit(Op::RunCleanups((loop_scope_depth + 1) as u16), span.line);
                }
                for _ in loop_try_depth..self.current().try_depth {
                    self.emit(Op::PopTry, span.line);
                }
                self.pop_stack_to_height(target_height, span.line);
                let j = self.emit(Op::Jump(0), span.line);
                self.current()
                    .loops
                    .last_mut()
                    .unwrap()
                    .continue_jumps
                    .push(j);
                Ok(())
            }
            Stmt::Needs { caps, span } => {
                let helper_idx = self.current().chunk.add_string_constant("__check_needs");
                self.emit(Op::GetGlobal(helper_idx), span.line);
                self.push_temps(1);
                let cap_count = caps.len();
                for (name, args) in caps {
                    let nidx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::Constant(nidx), span.line);
                    self.push_temps(1);
                    for a in args {
                        let aidx = self.current().chunk.add_string_constant(a);
                        self.emit(Op::Constant(aidx), span.line);
                        self.push_temps(1);
                    }
                    self.pop_temps(args.len());
                    self.emit(Op::MakeList(args.len() as u16), span.line);
                    self.push_temps(1);
                    self.pop_temps(2);
                    self.emit(Op::MakeList(2), span.line);
                    self.push_temps(1);
                }
                self.pop_temps(cap_count);
                self.emit(Op::MakeList(cap_count as u16), span.line);
                self.pop_temps(1);
                self.emit(Op::Call(1), span.line);
                self.emit(Op::Pop, span.line);
                Ok(())
            }
            Stmt::Defer { body, span } => {
                self.compile_function("<defer>".to_string(), &[], body, *span)?;
                let depth = self.current().scope_depth;
                self.current().cleanup_scopes.push(depth);
                self.emit(Op::PushDefer(depth as u16), span.line);
                Ok(())
            }
            Stmt::Every {
                interval,
                body,
                span,
            } => {
                let loop_start = self.current_ip();
                let stack_height = self.current().stack_height();
                let loop_try_depth = self.current().try_depth;
                let loop_scope_depth = self.current().scope_depth;
                self.current().loops.push(LoopContext {
                    break_stack_height: stack_height,
                    continue_stack_height: stack_height,
                    try_depth: loop_try_depth,
                    scope_depth: loop_scope_depth,
                    break_jumps: Vec::new(),
                    continue_jumps: Vec::new(),
                });

                self.compile_expr(body)?;
                self.emit(Op::Pop, span.line);

                let continue_target = self.current_ip();
                let ctx = self.current().loops.pop().unwrap();
                for j in ctx.continue_jumps {
                    self.current().chunk.patch_jump(j, continue_target);
                }

                let sleep_idx = self.current().chunk.add_string_constant("__sleep_checked");
                self.emit(Op::GetGlobal(sleep_idx), span.line);
                self.push_temps(1);
                self.compile_expr(interval)?;
                self.pop_temps(1);
                self.emit(Op::Call(1), span.line);
                self.emit(Op::Pop, span.line);

                self.emit(Op::Jump(loop_start), span.line);
                for j in ctx.break_jumps {
                    self.patch_jump(j);
                }
                Ok(())
            }
            Stmt::Spec { name, body, span } => {
                if !self.include_specs {
                    return Ok(());
                }
                let run_idx = self.current().chunk.add_string_constant("__run_spec");
                self.emit(Op::GetGlobal(run_idx), span.line);
                self.push_temps(1);
                let name_idx = self.current().chunk.add_string_constant(name);
                self.emit(Op::Constant(name_idx), span.line);
                self.push_temps(1);
                self.compile_function(format!("<spec {}>", name), &[], body, *span)?;
                self.pop_temps(2);
                self.emit(Op::Call(2), span.line);
                self.emit(Op::Pop, span.line);
                Ok(())
            }
            Stmt::Expr(expr) => {
                if let Some((slot, delta, line)) = self.match_local_inc_pop(expr) {
                    self.emit(Op::IncLocalPop(slot, delta), line);
                    return Ok(());
                }
                self.compile_expr(expr)?;
                self.emit(Op::Pop, expr.span().line);
                Ok(())
            }
        }
    }

    fn pop_stack_to_height(&mut self, target_height: usize, line: usize) {
        let frame = self.frames.last().unwrap();
        let cur_height = frame.stack_height();
        let mut ops = Vec::new();
        for pos in (target_height..cur_height).rev() {
            let captured = frame
                .locals
                .iter()
                .rev()
                .find(|l| l.slot as usize == pos)
                .is_some_and(|l| l.is_captured);
            if captured {
                ops.push(Op::CloseUpvalue);
            } else {
                ops.push(Op::Pop);
            }
        }
        for op in ops {
            self.emit(op, line);
        }
    }

    fn pop_locals_to_depth(&mut self, target_depth: usize, line: usize) {
        let frame = self.frames.last().unwrap();
        let mut ops = Vec::new();
        for local in frame.locals.iter().rev() {
            if local.depth <= target_depth {
                break;
            }
            if local.is_captured {
                ops.push(Op::CloseUpvalue);
            } else {
                ops.push(Op::Pop);
            }
        }
        for op in ops {
            self.emit(op, line);
        }
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), String> {
        self.compile_expr_inner(expr, false)
    }

    fn compile_expr_inner(&mut self, expr: &Expr, in_tail: bool) -> Result<(), String> {
        match expr {
            Expr::Int(v, span) => {
                if let Ok(imm) = i32::try_from(*v) {
                    self.emit(Op::ConstInt(imm), span.line);
                } else {
                    let idx = self.current().chunk.add_constant(Constant::Int(*v));
                    self.emit(Op::Constant(idx), span.line);
                }
                Ok(())
            }
            Expr::Float(v, span) => {
                let idx = self.current().chunk.add_constant(Constant::Float(*v));
                self.emit(Op::Constant(idx), span.line);
                Ok(())
            }
            Expr::Str(s, span) => {
                let idx = self.current().chunk.add_string_constant(s);
                self.emit(Op::Constant(idx), span.line);
                Ok(())
            }
            Expr::Bytes(b, span) => {
                let idx = self.current().chunk.add_bytes_constant(b);
                self.emit(Op::Constant(idx), span.line);
                Ok(())
            }
            Expr::Bool(b, span) => {
                self.emit(if *b { Op::True } else { Op::False }, span.line);
                Ok(())
            }
            Expr::Unit(span) => {
                self.emit(Op::Unit, span.line);
                Ok(())
            }
            Expr::Ident(name, span) => {
                let frame_idx = self.frames.len() - 1;
                if let Some((_, slot)) = Self::resolve_local(&self.frames[frame_idx], name) {
                    self.emit(Op::GetLocal(slot), span.line);
                } else if let Some(upval) = self.resolve_upvalue(frame_idx, name) {
                    self.emit(Op::GetUpvalue(upval), span.line);
                } else {
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::GetGlobal(idx), span.line);
                }
                Ok(())
            }
            Expr::List(items, span) => {
                if items.len() > u16::MAX as usize {
                    return Err("Too many list elements (max 65535)".to_string());
                }
                for item in items {
                    self.compile_expr(item)?;
                    self.push_temps(1);
                }
                self.pop_temps(items.len());
                self.emit(Op::MakeList(items.len() as u16), span.line);
                Ok(())
            }
            Expr::Dict(entries, span) => {
                if entries.len() > u16::MAX as usize {
                    return Err("Too many dictionary entries (max 65535)".to_string());
                }
                for (k, v) in entries {
                    self.compile_expr(k)?;
                    self.push_temps(1);
                    self.compile_expr(v)?;
                    self.push_temps(1);
                }
                self.pop_temps(entries.len() * 2);
                self.emit(Op::MakeDict(entries.len() as u16), span.line);
                Ok(())
            }
            Expr::Unary { op, expr, span } => {
                self.compile_expr(expr)?;
                match op {
                    UnaryOp::Neg => self.emit(Op::Neg, span.line),
                    UnaryOp::Not => self.emit(Op::Not, span.line),
                };
                Ok(())
            }
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => {
                if *op == BinaryOp::And {
                    self.compile_expr(left)?;
                    let end_jump = self.emit(Op::JumpIfFalsePeek(0), span.line);
                    self.emit(Op::Pop, span.line);
                    self.compile_expr(right)?;
                    self.patch_jump(end_jump);
                    return Ok(());
                }
                if *op == BinaryOp::Or {
                    self.compile_expr(left)?;
                    let end_jump = self.emit(Op::JumpIfTruePeek(0), span.line);
                    self.emit(Op::Pop, span.line);
                    self.compile_expr(right)?;
                    self.patch_jump(end_jump);
                    return Ok(());
                }

                if let Some(specialized) = self.match_local_binary(left, *op, right) {
                    self.emit(specialized, span.line);
                    return Ok(());
                }

                self.compile_expr(left)?;
                self.push_temps(1);
                self.compile_expr(right)?;
                self.pop_temps(1);
                match op {
                    BinaryOp::NotIn => {
                        self.emit(Op::In, span.line);
                        self.emit(Op::Not, span.line);
                    }
                    other => {
                        let op_code = match other {
                            BinaryOp::Add => Op::Add,
                            BinaryOp::Sub => Op::Sub,
                            BinaryOp::Mul => Op::Mul,
                            BinaryOp::Div => Op::Div,
                            BinaryOp::Mod => Op::Mod,
                            BinaryOp::Eq => Op::Eq,
                            BinaryOp::Ne => Op::Ne,
                            BinaryOp::Lt => Op::Lt,
                            BinaryOp::Le => Op::Le,
                            BinaryOp::Gt => Op::Gt,
                            BinaryOp::Ge => Op::Ge,
                            BinaryOp::In => Op::In,
                            BinaryOp::And | BinaryOp::Or | BinaryOp::NotIn => unreachable!(),
                        };
                        self.emit(op_code, span.line);
                    }
                }
                Ok(())
            }
            Expr::ChainedCmp(cmp) => {
                let first = &cmp.first;
                let comparisons = &cmp.comparisons;
                let span = cmp.span;
                self.compile_expr(first)?;
                let cur_slot = self.add_local(String::new());
                let mut fail_jumps = Vec::new();
                for (i, (op, rhs)) in comparisons.iter().enumerate() {
                    self.emit(Op::GetLocal(cur_slot), span.line);
                    self.push_temps(1);
                    self.compile_expr(rhs)?;
                    self.pop_temps(1);
                    if i + 1 < comparisons.len() {
                        self.emit(Op::Dup, span.line);
                        self.emit(Op::SetLocal(cur_slot), span.line);
                        self.emit(Op::Pop, span.line);
                    }
                    match op {
                        BinaryOp::Lt => {
                            self.emit(Op::Lt, span.line);
                        }
                        BinaryOp::Le => {
                            self.emit(Op::Le, span.line);
                        }
                        BinaryOp::Gt => {
                            self.emit(Op::Gt, span.line);
                        }
                        BinaryOp::Ge => {
                            self.emit(Op::Ge, span.line);
                        }
                        BinaryOp::In => {
                            self.emit(Op::In, span.line);
                        }
                        BinaryOp::NotIn => {
                            self.emit(Op::In, span.line);
                            self.emit(Op::Not, span.line);
                        }
                        _ => unreachable!(),
                    }
                    if i + 1 < comparisons.len() {
                        fail_jumps.push(self.emit(Op::JumpIfFalsePop(0), span.line));
                    } else {
                        self.emit(Op::SetLocal(cur_slot), span.line);
                        self.emit(Op::Pop, span.line);
                    }
                }
                let end_jump = self.emit(Op::Jump(0), span.line);
                for fj in fail_jumps {
                    self.patch_jump(fj);
                }
                self.emit(Op::False, span.line);
                self.emit(Op::SetLocal(cur_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.patch_jump(end_jump);
                self.current().locals.pop();
                Ok(())
            }
            Expr::Assign {
                target,
                op: Some(op),
                value,
                span,
            } => self.compile_compound_assign(target, *op, value, *span),
            Expr::Assign {
                target,
                op: None,
                value,
                span,
            } => match target.as_ref() {
                AssignTarget::Var(name) => {
                    self.compile_expr(value)?;
                    let frame_idx = self.frames.len() - 1;
                    if let Some((_, slot)) = Self::resolve_local(&self.frames[frame_idx], name) {
                        self.emit(Op::SetLocal(slot), span.line);
                    } else if let Some(upval) = self.resolve_upvalue(frame_idx, name) {
                        self.emit(Op::SetUpvalue(upval), span.line);
                    } else {
                        let idx = self.current().chunk.add_string_constant(name);
                        self.emit(Op::SetGlobal(idx), span.line);
                    }
                    Ok(())
                }
                AssignTarget::Index { object, index } => {
                    self.compile_expr(object)?;
                    self.push_temps(1);
                    self.compile_expr(index)?;
                    self.push_temps(1);
                    self.compile_expr(value)?;
                    self.pop_temps(2);
                    self.emit(Op::SetIndex, span.line);
                    Ok(())
                }
                AssignTarget::Attr { object, name } => {
                    self.compile_expr(object)?;
                    self.push_temps(1);
                    self.compile_expr(value)?;
                    self.pop_temps(1);
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::SetAttr(idx), span.line);
                    Ok(())
                }
            },
            Expr::Block { stmts, tail, span } => {
                if let Some(tail_expr) = tail {
                    if in_tail
                        && stmts.is_empty()
                        && self.current().try_depth == 0
                        && self.current().cleanup_scopes.is_empty()
                    {
                        return self.compile_expr_inner(tail_expr, true);
                    }
                    self.emit(Op::Unit, span.line);
                    let res_slot = self.add_local(String::new());
                    self.begin_scope();
                    for stmt in stmts {
                        self.compile_stmt(stmt)?;
                    }
                    self.compile_expr(tail_expr)?;
                    self.emit(Op::SetLocal(res_slot), tail_expr.span().line);
                    self.emit(Op::Pop, tail_expr.span().line);
                    self.end_scope(span.line);
                    self.current().locals.pop();
                } else {
                    self.begin_scope();
                    for stmt in stmts {
                        self.compile_stmt(stmt)?;
                    }
                    self.end_scope(span.line);
                    self.emit(Op::Unit, span.line);
                }
                Ok(())
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => {
                self.compile_expr(cond)?;
                let else_jump = self.emit(Op::JumpIfFalsePop(0), span.line);

                self.compile_expr_inner(then_branch, in_tail)?;
                let end_jump = self.emit(Op::Jump(0), span.line);

                self.patch_jump(else_jump);
                if let Some(else_expr) = else_branch {
                    self.compile_expr_inner(else_expr, in_tail)?;
                } else {
                    self.emit(Op::Unit, span.line);
                }
                self.patch_jump(end_jump);
                Ok(())
            }
            Expr::TryCatch {
                try_body,
                catch_var,
                catch_body,
                span,
            } => {
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());
                let setup_jump = self.emit(Op::SetupTry(0), span.line);

                self.current().try_depth += 1;
                self.compile_expr(try_body)?;
                self.current().try_depth -= 1;

                self.emit(Op::SetLocal(res_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.emit(Op::PopTry, span.line);
                let end_jump = self.emit(Op::Jump(0), span.line);

                self.patch_jump(setup_jump);
                self.begin_scope();
                let var_name = catch_var.clone().unwrap_or_else(|| "$err".to_string());
                self.add_local(var_name);
                self.compile_expr(catch_body)?;
                self.emit(Op::SetLocal(res_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.end_scope(span.line);

                self.patch_jump(end_jump);
                self.current().locals.pop();
                Ok(())
            }
            Expr::Throw { value, span } => {
                self.compile_expr(value)?;
                self.emit(Op::Throw, span.line);
                Ok(())
            }
            Expr::SafePipe(p) => {
                let (left, param, apply, span) = (&p.left, &p.param, &p.apply, p.span);
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());
                self.begin_scope();
                self.compile_expr(left)?;
                let val_slot = self.add_local(param.clone());
                self.emit(Op::GetLocal(val_slot), span.line);
                let apply_jump = self.emit(Op::JumpIfNotUnitPeek(0), span.line);
                self.emit(Op::Pop, span.line);
                let end_jump = self.emit(Op::Jump(0), span.line);
                self.patch_jump(apply_jump);
                self.emit(Op::Pop, span.line);
                self.compile_expr(apply)?;
                self.emit(Op::SetLocal(res_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.patch_jump(end_jump);
                self.end_scope(span.line);
                self.current().locals.pop();
                Ok(())
            }
            Expr::MapPipe { left, mapper, span } => {
                let idx = self.current().chunk.add_string_constant("__pipe_map");
                self.emit(Op::GetGlobal(idx), span.line);
                self.push_temps(1);
                self.compile_expr(left)?;
                self.push_temps(1);
                self.compile_expr(mapper)?;
                self.pop_temps(2);
                self.emit(Op::Call(2), span.line);
                Ok(())
            }
            Expr::PipeAssign(p) => {
                let (target, param, stages, span) = (&p.target, &p.param, &p.stages, p.span);
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());
                self.begin_scope();
                match target.as_ref() {
                    AssignTarget::Var(name) => {
                        let frame_idx = self.frames.len() - 1;
                        if let Some((_, slot)) = Self::resolve_local(&self.frames[frame_idx], name)
                        {
                            self.emit(Op::GetLocal(slot), span.line);
                        } else if let Some(upval) = self.resolve_upvalue(frame_idx, name) {
                            self.emit(Op::GetUpvalue(upval), span.line);
                        } else {
                            let idx = self.current().chunk.add_string_constant(name);
                            self.emit(Op::GetGlobal(idx), span.line);
                        }
                        let val_slot = self.add_local(param.clone());
                        for stage in stages {
                            self.compile_expr(stage)?;
                            self.emit(Op::SetLocal(val_slot), span.line);
                            self.emit(Op::Pop, span.line);
                        }
                        self.emit(Op::GetLocal(val_slot), span.line);
                        if let Some((_, slot)) = Self::resolve_local(&self.frames[frame_idx], name)
                        {
                            self.emit(Op::SetLocal(slot), span.line);
                        } else if let Some(upval) = self.resolve_upvalue(frame_idx, name) {
                            self.emit(Op::SetUpvalue(upval), span.line);
                        } else {
                            let idx = self.current().chunk.add_string_constant(name);
                            self.emit(Op::SetGlobal(idx), span.line);
                        }
                        self.emit(Op::SetLocal(res_slot), span.line);
                        self.emit(Op::Pop, span.line);
                    }
                    AssignTarget::Index { object, index } => {
                        self.compile_expr(object)?;
                        let obj_slot = self.add_local(String::new());
                        self.compile_expr(index)?;
                        let idx_slot = self.add_local(String::new());
                        self.emit(Op::GetLocal(obj_slot), span.line);
                        self.emit(Op::GetLocal(idx_slot), span.line);
                        self.emit(Op::GetIndex, span.line);
                        let val_slot = self.add_local(param.clone());
                        for stage in stages {
                            self.compile_expr(stage)?;
                            self.emit(Op::SetLocal(val_slot), span.line);
                            self.emit(Op::Pop, span.line);
                        }
                        self.emit(Op::GetLocal(obj_slot), span.line);
                        self.emit(Op::GetLocal(idx_slot), span.line);
                        self.emit(Op::GetLocal(val_slot), span.line);
                        self.emit(Op::SetIndex, span.line);
                        self.emit(Op::SetLocal(res_slot), span.line);
                        self.emit(Op::Pop, span.line);
                    }
                    AssignTarget::Attr { object, name } => {
                        self.compile_expr(object)?;
                        let obj_slot = self.add_local(String::new());
                        let attr_idx = self.current().chunk.add_string_constant(name);
                        self.emit(Op::GetLocal(obj_slot), span.line);
                        self.emit(Op::GetAttr(attr_idx), span.line);
                        let val_slot = self.add_local(param.clone());
                        for stage in stages {
                            self.compile_expr(stage)?;
                            self.emit(Op::SetLocal(val_slot), span.line);
                            self.emit(Op::Pop, span.line);
                        }
                        self.emit(Op::GetLocal(obj_slot), span.line);
                        self.emit(Op::GetLocal(val_slot), span.line);
                        self.emit(Op::SetAttr(attr_idx), span.line);
                        self.emit(Op::SetLocal(res_slot), span.line);
                        self.emit(Op::Pop, span.line);
                    }
                }
                self.end_scope(span.line);
                self.current().locals.pop();
                Ok(())
            }
            Expr::TaggedTemplate(t) => {
                let (tag, parts, exprs, expr_labels, span) =
                    (&t.tag, &t.parts, &t.exprs, &t.expr_labels, t.span);
                let frame_idx = self.frames.len() - 1;
                let local_slot = Self::resolve_local(&self.frames[frame_idx], tag).map(|(_, s)| s);
                let upval_slot = if local_slot.is_none() {
                    self.resolve_upvalue(frame_idx, tag)
                } else {
                    None
                };
                let has_local_tag = local_slot.is_some() || upval_slot.is_some();
                if tag == "log" || tag == "sql" {
                    let helper_idx = self.current().chunk.add_string_constant("__tag_eval");
                    self.emit(Op::GetGlobal(helper_idx), span.line);
                    self.push_temps(1);
                    let tag_idx = self.current().chunk.add_string_constant(tag);
                    self.emit(Op::Constant(tag_idx), span.line);
                    self.push_temps(1);
                    let part_count = parts.len();
                    for p in parts {
                        let pidx = self.current().chunk.add_string_constant(p);
                        self.emit(Op::Constant(pidx), span.line);
                        self.push_temps(1);
                    }
                    self.pop_temps(part_count);
                    self.emit(Op::MakeList(part_count as u16), span.line);
                    self.push_temps(1);
                    let expr_count = exprs.len();
                    for e in exprs {
                        self.compile_expr(e)?;
                        self.push_temps(1);
                    }
                    self.pop_temps(expr_count);
                    self.emit(Op::MakeList(expr_count as u16), span.line);
                    self.push_temps(1);
                    for lbl in expr_labels {
                        let lidx = self.current().chunk.add_string_constant(lbl);
                        self.emit(Op::Constant(lidx), span.line);
                        self.push_temps(1);
                    }
                    self.pop_temps(expr_labels.len());
                    self.emit(Op::MakeList(expr_labels.len() as u16), span.line);
                    let extra_arg = if let Some(slot) = local_slot {
                        self.push_temps(1);
                        self.emit(Op::GetLocal(slot), span.line);
                        1usize
                    } else if let Some(uv) = upval_slot {
                        self.push_temps(1);
                        self.emit(Op::GetUpvalue(uv), span.line);
                        1usize
                    } else {
                        0usize
                    };
                    self.pop_temps(4 + extra_arg);
                    self.emit(Op::Call(4 + extra_arg as u8), span.line);
                } else {
                    if has_local_tag {
                        if let Some(slot) = local_slot {
                            self.emit(Op::GetLocal(slot), span.line);
                        } else if let Some(upval) = upval_slot {
                            self.emit(Op::GetUpvalue(upval), span.line);
                        }
                    } else {
                        let idx = self.current().chunk.add_string_constant(tag);
                        self.emit(Op::GetGlobal(idx), span.line);
                    }
                    self.push_temps(1);
                    let part_count = parts.len();
                    for p in parts {
                        let pidx = self.current().chunk.add_string_constant(p);
                        self.emit(Op::Constant(pidx), span.line);
                        self.push_temps(1);
                    }
                    self.pop_temps(part_count);
                    self.emit(Op::MakeList(part_count as u16), span.line);
                    self.push_temps(1);
                    let expr_count = exprs.len();
                    for e in exprs {
                        self.compile_expr(e)?;
                        self.push_temps(1);
                    }
                    self.pop_temps(expr_count);
                    self.emit(Op::MakeList(expr_count as u16), span.line);
                    self.pop_temps(2);
                    self.emit(Op::Call(2), span.line);
                }
                Ok(())
            }
            Expr::With(w) => {
                let span = w.span;
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());
                self.begin_scope();
                let with_depth = self.current().scope_depth;
                self.compile_expr(&w.resource)?;
                let res_name = w.alias.clone().unwrap_or_else(|| "$with_res".to_string());
                self.add_local(res_name.clone());

                let defer_call = Expr::Call {
                    callee: Box::new(Expr::Ident("__close_resource".to_string(), span)),
                    args: vec![Expr::Ident(res_name, span)],
                    span,
                };
                self.compile_function("<with_close>".to_string(), &[], &defer_call, span)?;
                self.current().cleanup_scopes.push(with_depth);
                self.emit(Op::PushDefer(with_depth as u16), span.line);

                self.compile_expr(&w.body)?;
                self.emit(Op::SetLocal(res_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.end_scope(span.line);
                self.current().locals.pop();
                Ok(())
            }
            Expr::Sandbox(s) => {
                let span = s.span;
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());
                self.begin_scope();
                let sb_depth = self.current().scope_depth;
                if s.policy.is_empty() {
                    self.emit(Op::Unit, span.line);
                } else {
                    for (k, v) in &s.policy {
                        let kidx = self.current().chunk.add_string_constant(k);
                        self.emit(Op::Constant(kidx), span.line);
                        self.push_temps(1);
                        self.compile_expr(v)?;
                        self.push_temps(1);
                    }
                    self.pop_temps(s.policy.len() * 2);
                    self.emit(Op::MakeDict(s.policy.len() as u16), span.line);
                }
                self.current().cleanup_scopes.push(sb_depth);
                self.emit(Op::EnterSandbox(sb_depth as u16), span.line);

                self.compile_expr(&s.body)?;
                self.emit(Op::SetLocal(res_slot), span.line);
                self.emit(Op::Pop, span.line);
                self.end_scope(span.line);
                self.current().locals.pop();
                Ok(())
            }
            Expr::After { delay, body, span } => {
                let sleep_idx = self.current().chunk.add_string_constant("__sleep_checked");
                self.emit(Op::GetGlobal(sleep_idx), span.line);
                self.push_temps(1);
                self.compile_expr(delay)?;
                self.pop_temps(1);
                self.emit(Op::Call(1), span.line);
                self.emit(Op::Pop, span.line);
                self.compile_expr_inner(body, in_tail)
            }
            Expr::Match {
                subject,
                arms,
                span,
            } => {
                self.begin_scope();
                self.emit(Op::Unit, span.line);
                let res_slot = self.add_local(String::new());

                self.compile_expr(subject)?;
                let subj_slot = self.add_local("$match_subject".to_string());

                let mut end_jumps = Vec::new();
                let mut has_irrefutable = false;

                for arm in arms {
                    match &arm.pattern {
                        Pattern::Wildcard => {
                            if let Some(guard) = &arm.guard {
                                self.compile_expr(guard)?;
                                let next_arm = self.emit(Op::JumpIfFalsePop(0), arm.span.line);
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                                self.patch_jump(next_arm);
                            } else {
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                                has_irrefutable = true;
                                break;
                            }
                        }
                        Pattern::Ident(name) => {
                            self.begin_scope();
                            self.emit(Op::GetLocal(subj_slot), arm.span.line);
                            self.add_local(name.clone());
                            if let Some(guard) = &arm.guard {
                                self.compile_expr(guard)?;
                                let guard_fail = self.emit(Op::JumpIfFalsePop(0), arm.span.line);
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                let target_depth = self.current().scope_depth - 1;
                                self.pop_locals_to_depth(target_depth, arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                                self.patch_jump(guard_fail);
                                self.end_scope(arm.span.line);
                            } else {
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                self.end_scope(arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                                has_irrefutable = true;
                                break;
                            }
                        }
                        Pattern::Range {
                            start,
                            end,
                            inclusive,
                        } => {
                            self.emit(Op::GetLocal(subj_slot), arm.span.line);
                            self.emit(Op::IsNumeric, arm.span.line);
                            let type_fail = self.emit(Op::JumpIfFalsePop(0), arm.span.line);

                            self.emit(Op::GetLocal(subj_slot), arm.span.line);
                            let s_idx = self.current().chunk.add_constant(Constant::Int(*start));
                            self.emit(Op::Constant(s_idx), arm.span.line);
                            self.emit(Op::Ge, arm.span.line);
                            let low_fail = self.emit(Op::JumpIfFalsePop(0), arm.span.line);

                            self.emit(Op::GetLocal(subj_slot), arm.span.line);
                            let e_idx = self.current().chunk.add_constant(Constant::Int(*end));
                            self.emit(Op::Constant(e_idx), arm.span.line);
                            self.emit(if *inclusive { Op::Le } else { Op::Lt }, arm.span.line);
                            let high_fail = self.emit(Op::JumpIfFalsePop(0), arm.span.line);

                            let mut guard_fail = None;
                            if let Some(guard) = &arm.guard {
                                self.compile_expr(guard)?;
                                guard_fail = Some(self.emit(Op::JumpIfFalsePop(0), arm.span.line));
                            }
                            self.compile_expr(&arm.body)?;
                            self.emit(Op::SetLocal(res_slot), arm.span.line);
                            self.emit(Op::Pop, arm.span.line);
                            let j = self.emit(Op::Jump(0), arm.span.line);
                            end_jumps.push(j);

                            self.patch_jump(type_fail);
                            self.patch_jump(low_fail);
                            self.patch_jump(high_fail);
                            if let Some(gf) = guard_fail {
                                self.patch_jump(gf);
                            }
                        }
                        literal => {
                            self.emit(Op::GetLocal(subj_slot), arm.span.line);
                            match literal {
                                Pattern::Int(v) => {
                                    let idx = self.current().chunk.add_constant(Constant::Int(*v));
                                    self.emit(Op::Constant(idx), arm.span.line);
                                }
                                Pattern::Float(v) => {
                                    let idx =
                                        self.current().chunk.add_constant(Constant::Float(*v));
                                    self.emit(Op::Constant(idx), arm.span.line);
                                }
                                Pattern::Bool(b) => {
                                    self.emit(if *b { Op::True } else { Op::False }, arm.span.line);
                                }
                                Pattern::Str(s) => {
                                    let idx = self.current().chunk.add_string_constant(s);
                                    self.emit(Op::Constant(idx), arm.span.line);
                                }
                                Pattern::Unit => {
                                    self.emit(Op::Unit, arm.span.line);
                                }
                                Pattern::Wildcard | Pattern::Ident(_) | Pattern::Range { .. } => {
                                    unreachable!()
                                }
                            }
                            self.emit(Op::Eq, arm.span.line);
                            let next_arm = self.emit(Op::JumpIfFalsePop(0), arm.span.line);

                            if let Some(guard) = &arm.guard {
                                self.compile_expr(guard)?;
                                let guard_fail = self.emit(Op::JumpIfFalsePop(0), arm.span.line);
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                                self.patch_jump(guard_fail);
                            } else {
                                self.compile_expr(&arm.body)?;
                                self.emit(Op::SetLocal(res_slot), arm.span.line);
                                self.emit(Op::Pop, arm.span.line);
                                let j = self.emit(Op::Jump(0), arm.span.line);
                                end_jumps.push(j);
                            }

                            self.patch_jump(next_arm);
                        }
                    }
                }

                if !has_irrefutable {
                    self.emit(Op::Unit, span.line);
                    self.emit(Op::SetLocal(res_slot), span.line);
                    self.emit(Op::Pop, span.line);
                }

                for j in end_jumps {
                    self.patch_jump(j);
                }

                // Pop subj_slot (and keep res_slot on stack as match result)
                self.current().locals.pop();
                self.emit(Op::Pop, span.line);
                self.current().locals.pop();
                self.current().scope_depth -= 1;
                Ok(())
            }
            Expr::Fn { params, body, span } => {
                self.compile_function("<lambda>".to_string(), params, body, *span)
            }
            Expr::Call { callee, args, span } => {
                if args.len() > u8::MAX as usize {
                    return Err(format!(
                        "[{}:{}] Too many arguments (max 255)",
                        span.line, span.col
                    ));
                }
                self.compile_expr(callee)?;
                self.push_temps(1);
                for arg in args {
                    self.compile_expr(arg)?;
                    self.push_temps(1);
                }
                self.pop_temps(1 + args.len());
                if in_tail
                    && self.current().try_depth == 0
                    && self.current().cleanup_scopes.is_empty()
                {
                    self.emit(Op::TailCall(args.len() as u8), span.line);
                } else {
                    self.emit(Op::Call(args.len() as u8), span.line);
                }
                Ok(())
            }
            Expr::CallNamed(call) => {
                let callee = &call.callee;
                let pos_args = &call.pos_args;
                let named_args = &call.named_args;
                let span = call.span;
                if pos_args.len() + named_args.len() > u8::MAX as usize {
                    return Err(format!(
                        "[{}:{}] Too many arguments (max 255)",
                        span.line, span.col
                    ));
                }
                self.compile_expr(callee)?;
                self.push_temps(1);
                for arg in pos_args {
                    self.compile_expr(arg)?;
                    self.push_temps(1);
                }
                let mut kw_names = Vec::with_capacity(named_args.len());
                for (name, val) in named_args {
                    kw_names.push(name.clone());
                    self.compile_expr(val)?;
                    self.push_temps(1);
                }
                self.pop_temps(1 + pos_args.len() + named_args.len());
                let kw_idx = self.current().chunk.add_kw_list(kw_names);
                self.emit(Op::CallNamed(pos_args.len() as u8, kw_idx), span.line);
                Ok(())
            }
            Expr::StructInit(st) => {
                let target = &st.target;
                let fields = &st.fields;
                let spread = &st.spread;
                let span = st.span;
                self.compile_expr(target)?;
                self.push_temps(1);
                let mut pushed = 1usize;
                let has_spread = if let Some(sp) = spread {
                    self.compile_expr(sp)?;
                    self.push_temps(1);
                    pushed += 1;
                    1u16
                } else {
                    0u16
                };
                let mut names = Vec::with_capacity(fields.len());
                for (fname, fval) in fields {
                    names.push(fname.clone());
                    self.compile_expr(fval)?;
                    self.push_temps(1);
                    pushed += 1;
                }
                self.pop_temps(pushed);
                let kw_idx = self.current().chunk.add_kw_list(names);
                self.emit(Op::InitStruct(has_spread, kw_idx), span.line);
                Ok(())
            }
            Expr::Index {
                object,
                index,
                span,
            } => {
                self.compile_expr(object)?;
                self.push_temps(1);
                self.compile_expr(index)?;
                self.pop_temps(1);
                self.emit(Op::GetIndex, span.line);
                Ok(())
            }
            Expr::Slice {
                object,
                start,
                end,
                inclusive,
                span,
            } => {
                self.compile_expr(object)?;
                self.push_temps(1);
                if let Some(s) = start {
                    self.compile_expr(s)?;
                } else {
                    self.emit(Op::Unit, span.line);
                }
                self.push_temps(1);
                if let Some(e) = end {
                    self.compile_expr(e)?;
                } else {
                    self.emit(Op::Unit, span.line);
                }
                self.pop_temps(2);
                self.emit(Op::Slice(*inclusive), span.line);
                Ok(())
            }
            Expr::ListComp {
                expr,
                var,
                iter,
                cond,
                span,
            } => {
                self.emit(Op::MakeList(0), span.line);
                let out_slot = self.add_local(String::new());
                self.begin_scope();

                self.compile_expr(iter)?;
                let iter_slot = self.add_local("$comp_iter".to_string());
                self.emit(Op::Unit, span.line);
                let _idx_slot = self.add_local("$comp_idx".to_string());
                self.emit(Op::Unit, span.line);
                let _end_slot = self.add_local("$comp_end".to_string());

                let prep_jump = self.emit(Op::ForPrep(iter_slot, 0), span.line);
                let loop_start = self.current_ip();
                let exit_jump = self.emit(Op::ForIter(iter_slot, 0), span.line);

                self.begin_scope();
                self.add_local(var.clone());

                let skip_push = if let Some(c) = cond {
                    self.compile_expr(c)?;
                    Some(self.emit(Op::JumpIfFalsePop(0), span.line))
                } else {
                    None
                };

                let push_name = self.current().chunk.add_string_constant("push");
                self.emit(Op::GetGlobal(push_name), span.line);
                self.emit(Op::GetLocal(out_slot), span.line);
                self.push_temps(2);
                self.compile_expr(expr)?;
                self.pop_temps(2);
                self.emit(Op::Call(2), span.line);
                self.emit(Op::Pop, span.line);

                if let Some(sp) = skip_push {
                    self.patch_jump(sp);
                }
                self.end_scope(span.line);

                self.emit(Op::Jump(loop_start), span.line);
                self.patch_jump(prep_jump);
                self.patch_jump(exit_jump);
                self.end_scope(span.line);

                self.current().locals.pop();
                Ok(())
            }
            Expr::GetAttr { object, name, span } => {
                self.compile_expr(object)?;
                let idx = self.current().chunk.add_string_constant(name);
                self.emit(Op::GetAttr(idx), span.line);
                Ok(())
            }
            Expr::OptGetAttr { object, name, span } => {
                self.compile_expr(object)?;
                let idx = self.current().chunk.add_string_constant(name);
                self.emit(Op::OptGetAttr(idx), span.line);
                Ok(())
            }
            Expr::NullCoalesce { left, right, span } => {
                self.compile_expr(left)?;
                let end_jump = self.emit(Op::JumpIfNotUnitPeek(0), span.line);
                self.emit(Op::Pop, span.line);
                self.compile_expr(right)?;
                self.patch_jump(end_jump);
                Ok(())
            }
        }
    }

    fn compile_function(
        &mut self,
        name: String,
        params: &[FnParam],
        body: &Expr,
        span: Span,
    ) -> Result<(), String> {
        if params.len() > u8::MAX as usize {
            return Err(format!(
                "[{}:{}] Too many parameters (max 255)",
                span.line, span.col
            ));
        }
        let is_variadic = params.last().is_some_and(|p| p.is_variadic);
        let min_arity = params
            .iter()
            .filter(|p| !p.is_variadic && p.default.is_none())
            .count() as u8;
        let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let parent_is_main = self.frames.last().is_none_or(|f| f.is_main);
        let mut func_compiler = FunctionCompiler::new(name, params.len() as u8, parent_is_main);
        func_compiler.scope_depth = 1;
        for (i, p) in params.iter().enumerate() {
            func_compiler.locals.push(Local {
                name: p.name.clone(),
                depth: 1,
                slot: (i + 1) as u16,
                is_captured: false,
                is_pub: false,
            });
        }

        self.frames.push(func_compiler);
        for (i, p) in params.iter().enumerate() {
            if let Some(default_expr) = &p.default {
                let skip = self.emit(Op::JumpIfArgPresent(i as u16, 0), span.line);
                self.compile_expr(default_expr)?;
                self.emit(Op::SetLocal((i + 1) as u16), span.line);
                self.emit(Op::Pop, span.line);
                self.patch_jump(skip);
            }
        }
        self.compile_expr_inner(body, true)?;
        if !self.current().cleanup_scopes.is_empty() {
            self.emit(Op::RunCleanups(0), span.line);
        }
        self.emit(Op::Return, span.line);
        let compiled = self.frames.pop().unwrap();
        if compiled.chunk.limit_exceeded {
            return Err("Bytecode constant/local limit exceeded".to_string());
        }

        let upvalues = compiled.upvalues.clone();
        let proto = Rc::new(FunctionProto {
            ic_id: crate::bytecode::next_ic_id(),
            name: compiled.name,
            arity: compiled.arity,
            min_arity,
            is_variadic,
            param_names,
            local_count: Self::local_count(&compiled.chunk, compiled.arity as u16 + 1),
            upvalue_count: compiled.upvalues.len() as u16,
            is_main: compiled.is_main,
            chunk: compiled.chunk,
        });
        let idx = self.current().chunk.add_constant(Constant::Function(proto));
        let uv_idx = self.current().chunk.add_upvalue_list(upvalues);
        self.emit(Op::Closure(idx, uv_idx), span.line);
        Ok(())
    }

    fn compile_compound_assign(
        &mut self,
        target: &AssignTarget,
        op: BinaryOp,
        value: &Expr,
        span: Span,
    ) -> Result<(), String> {
        let op_code = match op {
            BinaryOp::Add => Op::Add,
            BinaryOp::Sub => Op::Sub,
            BinaryOp::Mul => Op::Mul,
            BinaryOp::Div => Op::Div,
            _ => unreachable!(),
        };
        match target {
            AssignTarget::Var(name) => {
                let frame_idx = self.frames.len() - 1;
                if let Some((_, slot)) = Self::resolve_local(&self.frames[frame_idx], name) {
                    self.emit(Op::GetLocal(slot), span.line);
                    self.push_temps(1);
                    self.compile_expr(value)?;
                    self.pop_temps(1);
                    self.emit(op_code, span.line);
                    self.emit(Op::SetLocal(slot), span.line);
                } else if let Some(upval) = self.resolve_upvalue(frame_idx, name) {
                    self.emit(Op::GetUpvalue(upval), span.line);
                    self.push_temps(1);
                    self.compile_expr(value)?;
                    self.pop_temps(1);
                    self.emit(op_code, span.line);
                    self.emit(Op::SetUpvalue(upval), span.line);
                } else {
                    let idx = self.current().chunk.add_string_constant(name);
                    self.emit(Op::GetGlobal(idx), span.line);
                    self.push_temps(1);
                    self.compile_expr(value)?;
                    self.pop_temps(1);
                    self.emit(op_code, span.line);
                    self.emit(Op::SetGlobal(idx), span.line);
                }
                Ok(())
            }
            AssignTarget::Index { object, index } => {
                self.compile_expr(object)?;
                self.push_temps(1);
                self.compile_expr(index)?;
                self.push_temps(1);
                self.emit(Op::Dup2, span.line);
                self.emit(Op::GetIndex, span.line);
                self.push_temps(1);
                self.compile_expr(value)?;
                self.pop_temps(3);
                self.emit(op_code, span.line);
                self.emit(Op::SetIndex, span.line);
                Ok(())
            }
            AssignTarget::Attr { object, name } => {
                self.compile_expr(object)?;
                self.push_temps(1);
                self.emit(Op::Dup, span.line);
                let idx = self.current().chunk.add_string_constant(name);
                self.emit(Op::GetAttr(idx), span.line);
                self.push_temps(1);
                self.compile_expr(value)?;
                self.pop_temps(2);
                self.emit(op_code, span.line);
                self.emit(Op::SetAttr(idx), span.line);
                Ok(())
            }
        }
    }

    fn match_local_slot(&self, name: &str) -> Option<u16> {
        let frame_idx = self.frames.len() - 1;
        Self::resolve_local(&self.frames[frame_idx], name).map(|(_, slot)| slot)
    }

    fn match_local_binary(&self, left: &Expr, op: BinaryOp, right: &Expr) -> Option<Op> {
        if let (Expr::Ident(name, _), Expr::Int(imm, _)) = (left, right) {
            if let (Some(slot), Ok(v)) = (self.match_local_slot(name), i32::try_from(*imm)) {
                return match op {
                    BinaryOp::Add => Some(Op::AddLocalInt(slot, v)),
                    BinaryOp::Sub => Some(Op::SubLocalInt(slot, v)),
                    BinaryOp::Lt => Some(Op::LtLocalInt(slot, v)),
                    BinaryOp::Le => Some(Op::LeLocalInt(slot, v)),
                    _ => None,
                };
            }
        }
        if let (Expr::Ident(lhs, _), Expr::Ident(rhs, _)) = (left, right) {
            if let (Some(a), Some(b)) = (self.match_local_slot(lhs), self.match_local_slot(rhs)) {
                return match op {
                    BinaryOp::Add => Some(Op::AddLocals(a, b)),
                    BinaryOp::Sub => Some(Op::SubLocals(a, b)),
                    BinaryOp::Lt => Some(Op::LtLocals(a, b)),
                    _ => None,
                };
            }
        }
        None
    }

    fn match_local_inc_pop(&self, expr: &Expr) -> Option<(u16, i32, usize)> {
        let Expr::Assign {
            target,
            op,
            value,
            span,
        } = expr
        else {
            return None;
        };
        let AssignTarget::Var(target_name) = target.as_ref() else {
            return None;
        };
        let slot = self.match_local_slot(target_name)?;
        match op {
            Some(BinaryOp::Add) => {
                if let Expr::Int(k, _) = value.as_ref() {
                    return i32::try_from(*k).ok().map(|d| (slot, d, span.line));
                }
                None
            }
            Some(BinaryOp::Sub) => {
                if let Expr::Int(k, _) = value.as_ref() {
                    return k
                        .checked_neg()
                        .and_then(|n| i32::try_from(n).ok())
                        .map(|d| (slot, d, span.line));
                }
                None
            }
            None => {
                if let Expr::Binary {
                    left,
                    op: bin_op,
                    right,
                    ..
                } = value.as_ref()
                {
                    if let (Expr::Ident(lhs_name, _), Expr::Int(k, _)) =
                        (left.as_ref(), right.as_ref())
                    {
                        if lhs_name == target_name {
                            return match bin_op {
                                BinaryOp::Add => {
                                    i32::try_from(*k).ok().map(|d| (slot, d, span.line))
                                }
                                BinaryOp::Sub => k
                                    .checked_neg()
                                    .and_then(|n| i32::try_from(n).ok())
                                    .map(|d| (slot, d, span.line)),
                                _ => None,
                            };
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}
