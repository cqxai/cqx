//! Exhaustive typed AST traversal. New parser variants require a compile fix.
use super::*;

impl<W: Write> Extractor<'_, W> {
    pub(super) fn expr(&mut self, e: &Expression) -> io::Result<()> {
        match e {
            Expression::Call(c) => self.call(c)?,
            Expression::Index(x) => {
                self.expr(&x.left)?;
                self.expr(&x.index)?;
            }
            Expression::IndexList(x) => {
                self.expr(&x.left)?;
                self.expressions(&x.indices)?;
            }
            Expression::Slice(x) => {
                self.expr(&x.left)?;
                for e in x.index.iter().flatten() {
                    self.expr(e)?;
                }
            }
            Expression::Ident(_) | Expression::BasicLit(_) => {}
            Expression::FuncLit(f) => self.function(&f.typ, None, Some(&f.body))?,
            Expression::Ellipsis(x) => {
                if let Some(e) = &x.elt {
                    self.expr(e)?;
                }
            }
            Expression::Selector(x) => self.expr(&x.x)?,
            Expression::Range(x) => self.expr(&x.right)?,
            Expression::Star(x) => self.expr(&x.right)?,
            Expression::Paren(x) => self.expr(&x.expr)?,
            Expression::TypeAssert(x) => {
                self.expr(&x.left)?;
                if let Some(e) = &x.right {
                    self.expr(e)?;
                }
            }
            Expression::CompositeLit(x) => {
                self.expr(&x.typ)?;
                self.literal(&x.val)?;
            }
            Expression::List(v) => self.expressions(v)?,
            Expression::Operation(x) => {
                self.expr(&x.x)?;
                if let Some(e) = &x.y {
                    self.expr(e)?;
                }
            }
            Expression::TypeMap(x) => {
                self.expr(&x.key)?;
                self.expr(&x.val)?;
            }
            Expression::TypeArray(x) => {
                self.expr(&x.len)?;
                self.expr(&x.typ)?;
            }
            Expression::TypeSlice(x) => self.expr(&x.typ)?,
            Expression::TypeFunction(x) => self.fields(&x.params)?,
            Expression::TypeStruct(x) => {
                for f in &x.fields {
                    self.expr(&f.typ)?;
                }
            }
            Expression::TypeChannel(x) => self.expr(&x.typ)?,
            Expression::TypePointer(x) => self.expr(&x.typ)?,
            Expression::TypeInterface(x) => self.fields(&x.methods)?,
        }
        Ok(())
    }
    fn expressions(&mut self, values: &[Expression]) -> io::Result<()> {
        for e in values {
            self.expr(e)?;
        }
        Ok(())
    }
    fn fields(&mut self, fields: &FieldList) -> io::Result<()> {
        for f in &fields.list {
            self.expr(&f.typ)?;
        }
        Ok(())
    }
    fn literal(&mut self, value: &LiteralValue) -> io::Result<()> {
        for e in &value.values {
            if let Some(k) = &e.key {
                self.element(k)?;
            }
            self.element(&e.val)?;
        }
        Ok(())
    }
    fn element(&mut self, e: &Element) -> io::Result<()> {
        match e {
            Element::Expr(e) => self.expr(e),
            Element::LitValue(v) => self.literal(v),
        }
    }
    pub(super) fn block(&mut self, block: &BlockStmt) -> io::Result<()> {
        self.scopes.push(BTreeMap::new());
        for stmt in &block.list {
            self.stmt(stmt)?;
        }
        self.scopes.pop();
        Ok(())
    }
    fn optional(&mut self, stmt: &Option<Box<Statement>>) -> io::Result<()> {
        if let Some(stmt) = stmt {
            self.stmt(stmt)?;
        }
        Ok(())
    }
    pub(super) fn discarded(
        &mut self,
        e: &Expression,
        slots: Option<&[Expression]>,
    ) -> io::Result<()> {
        let errors = self.call_results(e);
        if errors.iter().enumerate().any(|(i, error)| {
            *error && slots.is_none_or(|v| v.get(i).is_some_and(|e| ident(e) == Some("_")))
        }) {
            self.rule(
                "discarded-check",
                EdgeKind::Discards,
                e.pos(),
                "known error result discarded",
            )?;
        }
        Ok(())
    }
    fn assignment(&mut self, x: &AssignStmt) -> io::Result<()> {
        if x.right.len() == 1 {
            self.discarded(&x.right[0], Some(&x.left))?;
        } else {
            for (left, right) in x.left.iter().zip(&x.right) {
                if ident(left) == Some("_") {
                    self.discarded(right, None)?;
                }
            }
        }
        self.expressions(&x.right)?;
        let results = if x.right.len() == 1 {
            self.call_results(&x.right[0])
        } else {
            Vec::new()
        };
        for (i, left) in x.left.iter().enumerate() {
            if let Some(name) = ident(left) {
                let value = if results.get(i) == Some(&true) {
                    Binding::Error
                } else {
                    x.right
                        .get(i)
                        .map(|e| self.value(e))
                        .unwrap_or(Binding::Unknown)
                };
                // In a := declaration only new names are created; existing
                // names in the current scope are assignments.
                self.set(name, value, x.op == Operator::Define);
            } else {
                self.expr(left)?;
            }
        }
        Ok(())
    }
    fn empty_error(&self, x: &IfStmt) -> bool {
        let Expression::Operation(o) = inner(&x.cond) else {
            return false;
        };
        let Some(y) = &o.y else {
            return false;
        };
        o.op == Operator::NotEqual
            && ((ident(y) == Some("nil")
                && ident(&o.x).is_some_and(|n| matches!(self.lookup(n), Binding::Error)))
                || (ident(&o.x) == Some("nil")
                    && ident(y).is_some_and(|n| matches!(self.lookup(n), Binding::Error))))
            && x.body.list.iter().all(|s| matches!(s, Statement::Empty(_)))
            && !self.ast.comments.iter().any(|c| {
                c.pos > x.body.pos.0
                    && c.pos < x.body.pos.1
                    && !c.text.trim_matches('/').trim().is_empty()
            })
    }
    fn cases(&mut self, block: &CaseBlock) -> io::Result<()> {
        for case in &block.body {
            self.scopes.push(BTreeMap::new());
            self.expressions(&case.list)?;
            for stmt in case.body.iter() {
                self.stmt(stmt)?;
            }
            self.scopes.pop();
        }
        Ok(())
    }
    fn stmt(&mut self, stmt: &Statement) -> io::Result<()> {
        match stmt {
            Statement::Go(x) => self.call(&x.call)?,
            Statement::Defer(x) => self.call(&x.call)?, // best-effort deferred cleanup is legitimate
            Statement::Expr(x) => {
                self.discarded(&x.expr, None)?;
                self.expr(&x.expr)?;
            }
            Statement::Assign(x) => self.assignment(x)?,
            Statement::If(x) => {
                self.scopes.push(BTreeMap::new());
                self.optional(&x.init)?;
                if self.empty_error(x) {
                    self.rule(
                        "discarded-check",
                        EdgeKind::Discards,
                        x.pos,
                        "empty error branch without a reason",
                    )?;
                }
                self.expr(&x.cond)?;
                self.block(&x.body)?;
                self.optional(&x.else_)?;
                self.scopes.pop();
            }
            Statement::For(x) => {
                self.scopes.push(BTreeMap::new());
                self.optional(&x.init)?;
                self.optional(&x.cond)?;
                self.optional(&x.post)?;
                self.block(&x.body)?;
                self.scopes.pop();
            }
            Statement::Range(x) => {
                self.expr(&x.expr)?;
                self.scopes.push(BTreeMap::new());
                for e in [&x.key, &x.value].into_iter().flatten() {
                    if let Some(n) = ident(e) {
                        self.set(
                            n,
                            Binding::Unknown,
                            x.op.is_some_and(|(_, op)| op == Operator::Define),
                        );
                    }
                }
                self.block(&x.body)?;
                self.scopes.pop();
            }
            Statement::Send(x) => {
                self.expr(&x.chan)?;
                self.expr(&x.value)?;
            }
            Statement::Block(x) => self.block(x)?,
            Statement::Empty(_) | Statement::Branch(_) => {}
            Statement::Label(x) => self.stmt(&x.stmt)?,
            Statement::IncDec(x) => self.expr(&x.expr)?,
            Statement::Return(x) => self.expressions(&x.ret)?,
            Statement::Switch(x) => {
                self.scopes.push(BTreeMap::new());
                self.optional(&x.init)?;
                if let Some(e) = &x.tag {
                    self.expr(e)?;
                }
                self.cases(&x.block)?;
                self.scopes.pop();
            }
            Statement::TypeSwitch(x) => {
                self.scopes.push(BTreeMap::new());
                self.optional(&x.init)?;
                self.optional(&x.tag)?;
                self.cases(&x.block)?;
                self.scopes.pop();
            }
            Statement::Select(x) => {
                for case in &x.body.body {
                    self.scopes.push(BTreeMap::new());
                    self.optional(&case.comm)?;
                    for stmt in case.body.iter() {
                        self.stmt(stmt)?;
                    }
                    self.scopes.pop();
                }
            }
            Statement::Declaration(x) => match x {
                DeclStmt::Variable(d) => {
                    for s in &d.specs {
                        self.variables(&s.name, s.typ.as_ref(), &s.values)?;
                    }
                }
                DeclStmt::Const(d) => {
                    for s in &d.specs {
                        self.variables(&s.name, s.typ.as_ref(), &s.values)?;
                    }
                }
                DeclStmt::Type(d) => {
                    for s in &d.specs {
                        self.bind(&s.name.name, Binding::Unknown);
                        self.expr(&s.typ)?;
                    }
                }
            },
        }
        Ok(())
    }
}
