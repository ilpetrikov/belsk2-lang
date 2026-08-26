package interpreter

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"
)

type breakSignal struct{}
type continueSignal struct{}
type returnSignal struct{ val Value }

type Value struct {
	Type   string
	Str    string
	Num    float64
	Bool   bool
	Array  []Value
	Obj    map[string]Value
	Fn     *FnValue
	IsNull bool
}

type FnValue struct {
	Params     []TypedParam
	ReturnType BType
	HasRetType bool
	Body       ASTNode
	Env        *Scope
}

type TypedVar struct {
	Value   Value
	VarType BType
}

type Scope struct {
	vars   map[string]TypedVar
	parent *Scope
}

func NewScope(parent *Scope) *Scope {
	return &Scope{vars: make(map[string]TypedVar), parent: parent}
}

func (s *Scope) Get(name string) (Value, bool) {
	if tv, ok := s.vars[name]; ok {
		return tv.Value, true
	}
	if s.parent != nil {
		return s.parent.Get(name)
	}
	return Value{IsNull: true}, false
}

func (s *Scope) Set(name string, v Value) {
	if tv, ok := s.vars[name]; ok {
		if tv.VarType == TYPE_BEL && (v.Type != "number" || v.Num > 1000) {
			fmt.Fprintf(os.Stderr, "bel error: value %s exceeds maximum of 1000 for variable '%s'\n", valStr(v), name)
			os.Exit(1)
		}
		if tv.VarType == TYPE_STER && v.Type != "string" {
			fmt.Fprintf(os.Stderr, "ster error: expected string, got %s for variable '%s'\n", v.Type, name)
			os.Exit(1)
		}
		if !typeCompatible(tv.VarType, runtimeTypeOf(v)) {
			typeMismatchError(tv.VarType, runtimeTypeOf(v), 0, 0, "assignment to '"+name+"'")
		}
		if isNumericType(tv.VarType) {
			if tv.VarType == TYPE_INT && v.Type == "number" {
				v.Num = float64(int64(v.Num))
			}
		}
		tv.Value = v
		s.vars[name] = tv
		return
	}
	if s.parent != nil {
		if _, ok := s.parent.Get(name); ok {
			s.parent.Set(name, v)
			return
		}
	}
	s.vars[name] = TypedVar{Value: v, VarType: runtimeTypeOf(v)}
}

func (s *Scope) Define(name string, v Value) {
	s.vars[name] = TypedVar{Value: v, VarType: runtimeTypeOf(v)}
}

func (s *Scope) DefineTyped(name string, v Value, t BType) {
	if !typeCompatible(t, runtimeTypeOf(v)) {
		typeMismatchError(t, runtimeTypeOf(v), 0, 0, "declaration of '"+name+"'")
	}
	if t == TYPE_BEL && (v.Type != "number" || v.Num > 1000) {
		fmt.Fprintf(os.Stderr, "bel error: value %s exceeds maximum of 1000 for variable '%s'\n", valStr(v), name)
		os.Exit(1)
	}
	if isNumericType(t) && t == TYPE_INT && v.Type == "number" {
		v.Num = float64(int64(v.Num))
	}
	s.vars[name] = TypedVar{Value: v, VarType: t}
}

type Interpreter struct {
	global *Scope
	output io.Writer
	idBank map[int]Value
}

func NewInterpreter() *Interpreter {
	return &Interpreter{global: NewScope(nil), output: os.Stdout, idBank: make(map[int]Value)}
}

func (interp *Interpreter) SetOutput(w io.Writer) {
	interp.output = w
}

func (interp *Interpreter) Run(prog *Program) {
	interp.execStmts(prog.Stmts, interp.global)
}

func (interp *Interpreter) execStmts(stmts []ASTNode, scope *Scope) {
	for _, s := range stmts {
		interp.exec(s, scope)
	}
}

func (interp *Interpreter) exec(node ASTNode, scope *Scope) Value {
	switch n := node.(type) {
	case *Program:
		interp.execStmts(n.Stmts, scope)
	case *BlockStmt:
		bs := NewScope(scope)
		interp.execStmts(n.Body, bs)
	case *ExprStmt:
		return interp.eval(n.Expr, scope)
	case *VarDecl:
		val := interp.eval(n.Value, scope)
		if n.IsBel {
			if val.Type != "number" || val.Num > 1000 {
				fmt.Fprintf(os.Stderr, "bel error: value %s exceeds maximum of 1000 for variable '%s'\n", valStr(val), n.Name)
				os.Exit(1)
			}
			scope.DefineTyped(n.Name, val, TYPE_BEL)
		} else if n.IsSter {
			if val.Type != "string" {
				fmt.Fprintf(os.Stderr, "ster error: expected string, got %s for variable '%s'\n", val.Type, n.Name)
				os.Exit(1)
			}
			scope.DefineTyped(n.Name, val, TYPE_STER)
		} else if n.HasType {
			scope.DefineTyped(n.Name, val, n.VarType)
		} else {
			scope.Define(n.Name, val)
		}
	case *IdbDecl:
		val := interp.eval(n.Value, scope)
		interp.idBank[n.Id] = val
	case *Assign:
		val := interp.eval(n.Value, scope)
		interp.setTarget(n.Target, val, scope)
	case *FnDecl:
		fn := &FnValue{
			Params:     n.Params,
			ReturnType: n.ReturnType,
			HasRetType: n.HasRetType,
			Body:       n.Body,
			Env:        scope,
		}
		scope.Define(n.Name, Value{Type: "function", Fn: fn})
	case *ReturnStmt:
		var val Value
		if n.Value != nil {
			val = interp.eval(n.Value, scope)
		}
		panic(returnSignal{val: val})
	case *BreakStmt:
		panic(breakSignal{})
	case *ContinueStmt:
		panic(continueSignal{})
	case *IfStmt:
		cond := interp.eval(n.Cond, scope)
		if isTruthy(cond) {
			interp.exec(n.Body, scope)
		} else if n.Else != nil {
			interp.exec(n.Else, scope)
		}
	case *WhileStmt:
		for {
			cond := interp.eval(n.Cond, scope)
			if !isTruthy(cond) {
				break
			}
			func() {
				defer func() {
					if r := recover(); r != nil {
						if _, ok := r.(breakSignal); ok {
							return
						}
						if _, ok := r.(continueSignal); ok {
							return
						}
						panic(r)
					}
				}()
				interp.exec(n.Body, scope)
			}()
		}
	case *ForStmt:
		iter := interp.eval(n.Iter, scope)
		if iter.Type == "array" {
			for _, item := range iter.Array {
				ls := NewScope(scope)
				ls.Define(n.Var, item)
				func() {
					defer func() {
						if r := recover(); r != nil {
							if _, ok := r.(breakSignal); ok {
								panic(breakSignal{})
							}
							if _, ok := r.(continueSignal); ok {
								return
							}
							panic(r)
						}
					}()
					interp.exec(n.Body, ls)
				}()
			}
		} else if iter.Type == "string" {
			for _, ch := range iter.Str {
				ls := NewScope(scope)
				ls.Define(n.Var, Value{Type: "string", Str: string(ch)})
				func() {
					defer func() {
						if r := recover(); r != nil {
							if _, ok := r.(breakSignal); ok {
								panic(breakSignal{})
							}
							if _, ok := r.(continueSignal); ok {
								return
							}
							panic(r)
						}
					}()
					interp.exec(n.Body, ls)
				}()
			}
		}
	}
	return Value{}
}

func (interp *Interpreter) setTarget(target ASTNode, val Value, scope *Scope) {
	switch t := target.(type) {
	case *Ident:
		scope.Set(t.Name, val)
	case *DotExpr:
		obj := interp.eval(t.Left, scope)
		if obj.Type == "object" {
			obj.Obj[t.Right] = val
		}
	case *IndexExpr:
		obj := interp.eval(t.Obj, scope)
		idx := interp.eval(t.Index, scope)
		if obj.Type == "array" && idx.Type == "number" {
			i := int(idx.Num)
			if i >= 0 && i < len(obj.Array) {
				obj.Array[i] = val
			}
		}
	default:
		fmt.Fprintln(os.Stderr, "invalid assignment target")
		os.Exit(1)
	}
}

func (interp *Interpreter) eval(node ASTNode, scope *Scope) Value {
	switch n := node.(type) {
	case *StringLit:
		return Value{Type: "string", Str: n.Value}
	case *NumLit:
		num, err := strconv.ParseFloat(n.Value, 64)
		if err != nil {
			num = 0
		}
		return Value{Type: "number", Num: num}
	case *BoolLit:
		return Value{Type: "bool", Bool: n.Value}
	case *Ident:
		if n.Name == "null" {
			return Value{IsNull: true}
		}
		v, ok := scope.Get(n.Name)
		if !ok {
			fmt.Fprintf(os.Stderr, "undefined variable: %s\n", n.Name)
			os.Exit(1)
		}
		return v
	case *ArrayLit:
		var elems []Value
		for _, el := range n.Elems {
			elems = append(elems, interp.eval(el, scope))
		}
		return Value{Type: "array", Array: elems}
	case *BinExpr:
		left := interp.eval(n.Left, scope)
		if n.Op == "&&" {
			if !isTruthy(left) {
				return Value{Type: "bool", Bool: false}
			}
			return Value{Type: "bool", Bool: isTruthy(interp.eval(n.Right, scope))}
		}
		if n.Op == "||" {
			if isTruthy(left) {
				return Value{Type: "bool", Bool: true}
			}
			return Value{Type: "bool", Bool: isTruthy(interp.eval(n.Right, scope))}
		}
		right := interp.eval(n.Right, scope)
		return evalBinOp(n.Op, left, right)
	case *UnaryExpr:
		val := interp.eval(n.Expr, scope)
		if n.Op == "-" {
			return Value{Type: "number", Num: -val.Num}
		}
		if n.Op == "!" {
			return Value{Type: "bool", Bool: !isTruthy(val)}
		}
	case *CallExpr:
		return interp.evalCall(n, scope)
	case *DotExpr:
		obj := interp.eval(n.Left, scope)
		if obj.Type == "object" {
			if v, ok := obj.Obj[n.Right]; ok {
				return v
			}
		}
		fmt.Fprintf(os.Stderr, "undefined property: %s\n", n.Right)
		os.Exit(1)
	case *IndexExpr:
		obj := interp.eval(n.Obj, scope)
		idx := interp.eval(n.Index, scope)
		if obj.Type == "array" && idx.Type == "number" {
			i := int(idx.Num)
			if i < 0 || i >= len(obj.Array) {
				fmt.Fprintf(os.Stderr, "index out of bounds: %d\n", i)
				os.Exit(1)
			}
			return obj.Array[i]
		}
		if obj.Type == "string" && idx.Type == "number" {
			i := int(idx.Num)
			if i < 0 || i >= len(obj.Str) {
				fmt.Fprintf(os.Stderr, "index out of bounds: %d\n", i)
				os.Exit(1)
			}
			return Value{Type: "string", Str: string(obj.Str[i])}
		}
	}
	return Value{}
}

func (interp *Interpreter) evalCall(call *CallExpr, scope *Scope) Value {
	args := make([]Value, len(call.Args))
	for i, a := range call.Args {
		args[i] = interp.eval(a, scope)
	}

	switch call.Name {
	case "prinb":
		if len(args) > 0 {
			v := args[0]
			if v.Type == "number" {
				if bv, ok := interp.idBank[int(v.Num)]; ok {
					v = bv
				}
			}
			fmt.Fprintln(interp.output, valStr(v))
		}
		return Value{}
	case "reab":
		if len(args) > 0 && args[0].Type == "number" {
			id := int(args[0].Num)
			reader := bufio.NewReader(os.Stdin)
			line, _ := reader.ReadString('\n')
			line = strings.TrimRight(line, "\r\n")
			v := Value{Type: "string", Str: line}
			interp.idBank[id] = v
			return v
		}
		return Value{}
	case "input":
		if len(args) > 0 {
			fmt.Fprint(interp.output, valStr(args[0]))
		}
		reader := bufio.NewReader(os.Stdin)
		line, _ := reader.ReadString('\n')
		line = strings.TrimRight(line, "\r\n")
		return Value{Type: "string", Str: line}
	case "len":
		if len(args) > 0 {
			if args[0].Type == "string" {
				return Value{Type: "number", Num: float64(len(args[0].Str))}
			}
			if args[0].Type == "array" {
				return Value{Type: "number", Num: float64(len(args[0].Array))}
			}
		}
	case "str":
		if len(args) > 0 {
			return Value{Type: "string", Str: valStr(args[0])}
		}
	case "num":
		if len(args) > 0 && args[0].Type == "string" {
			num, err := strconv.ParseFloat(args[0].Str, 64)
			if err != nil {
				num = 0
			}
			return Value{Type: "number", Num: num}
		}
	case "int":
		if len(args) > 0 {
			if args[0].Type == "number" {
				return Value{Type: "number", Num: float64(int64(args[0].Num))}
			}
			if args[0].Type == "string" {
				num, err := strconv.ParseFloat(args[0].Str, 64)
				if err != nil {
					num = 0
				}
				return Value{Type: "number", Num: float64(int64(num))}
			}
		}
	case "float":
		if len(args) > 0 {
			if args[0].Type == "number" {
				return args[0]
			}
			if args[0].Type == "string" {
				num, err := strconv.ParseFloat(args[0].Str, 64)
				if err != nil {
					num = 0
				}
				return Value{Type: "number", Num: num}
			}
		}
	case "bool":
		if len(args) > 0 {
			return Value{Type: "bool", Bool: isTruthy(args[0])}
		}
	case "push":
		if len(args) == 2 && args[0].Type == "array" {
			args[0].Array = append(args[0].Array, args[1])
			return args[0]
		}
	case "pop":
		if len(args) == 1 && args[0].Type == "array" && len(args[0].Array) > 0 {
			v := args[0].Array[len(args[0].Array)-1]
			args[0].Array = args[0].Array[:len(args[0].Array)-1]
			return v
		}
	case "substr":
		if len(args) == 3 && args[0].Type == "string" && args[1].Type == "number" && args[2].Type == "number" {
			s := args[0].Str
			start := int(args[1].Num)
			length := int(args[2].Num)
			if start < 0 {
				start = 0
			}
			if start >= len(s) {
				return Value{Type: "string", Str: ""}
			}
			end := start + length
			if end > len(s) {
				end = len(s)
			}
			return Value{Type: "string", Str: s[start:end]}
		}
	case "type":
		if len(args) > 0 {
			return Value{Type: "string", Str: args[0].Type}
		}
	default:
		fnVal, ok := scope.Get(call.Name)
		if !ok {
			fmt.Fprintf(os.Stderr, "undefined function: %s\n", call.Name)
			os.Exit(1)
		}
		if fnVal.Type != "function" || fnVal.Fn == nil {
			fmt.Fprintf(os.Stderr, "%s is not a function\n", call.Name)
			os.Exit(1)
		}
		fn := fnVal.Fn
		fnScope := NewScope(fn.Env)
		for i, param := range fn.Params {
			if i < len(args) {
				if !typeCompatible(param.Type, runtimeTypeOf(args[i])) {
					typeMismatchError(param.Type, runtimeTypeOf(args[i]), 0, 0,
						"argument '"+param.Name+"' of function '"+call.Name+"'")
				}
				if isNumericType(param.Type) && param.Type == TYPE_INT && args[i].Type == "number" {
					args[i].Num = float64(int64(args[i].Num))
				}
				fnScope.DefineTyped(param.Name, args[i], param.Type)
			}
		}
		var result Value
		func() {
			defer func() {
				if r := recover(); r != nil {
					if rs, ok := r.(returnSignal); ok {
						result = rs.val
						if fn.HasRetType && !typeCompatible(fn.ReturnType, runtimeTypeOf(result)) {
							typeMismatchError(fn.ReturnType, runtimeTypeOf(result), 0, 0,
								"return value of function '"+call.Name+"'")
						}
						return
					}
					panic(r)
				}
			}()
			interp.exec(fn.Body, fnScope)
		}()
		return result
	}
	return Value{}
}

func evalBinOp(op string, left, right Value) Value {
	switch op {
	case "+":
		if left.Type == "string" || right.Type == "string" {
			return Value{Type: "string", Str: valStr(left) + valStr(right)}
		}
		return Value{Type: "number", Num: left.Num + right.Num}
	case "-":
		return Value{Type: "number", Num: left.Num - right.Num}
	case "*":
		return Value{Type: "number", Num: left.Num * right.Num}
	case "/":
		if right.Num == 0 {
			fmt.Fprintln(os.Stderr, "division by zero")
			os.Exit(1)
		}
		return Value{Type: "number", Num: left.Num / right.Num}
	case "%":
		return Value{Type: "number", Num: float64(int(left.Num) % int(right.Num))}
	case "==":
		return Value{Type: "bool", Bool: valEqual(left, right)}
	case "!=":
		return Value{Type: "bool", Bool: !valEqual(left, right)}
	case "<":
		return Value{Type: "bool", Bool: left.Num < right.Num}
	case ">":
		return Value{Type: "bool", Bool: left.Num > right.Num}
	case "<=":
		return Value{Type: "bool", Bool: left.Num <= right.Num}
	case ">=":
		return Value{Type: "bool", Bool: left.Num >= right.Num}
	}
	fmt.Fprintf(os.Stderr, "unknown operator: %s\n", op)
	os.Exit(1)
	return Value{}
}

func isTruthy(v Value) bool {
	if v.IsNull {
		return false
	}
	switch v.Type {
	case "bool":
		return v.Bool
	case "number":
		return v.Num != 0
	case "string":
		return v.Str != ""
	case "array":
		return len(v.Array) > 0
	}
	return true
}

func valEqual(a, b Value) bool {
	if a.IsNull && b.IsNull {
		return true
	}
	if a.IsNull || b.IsNull {
		return false
	}
	if a.Type != b.Type {
		if isNumericType(runtimeTypeOf(a)) && isNumericType(runtimeTypeOf(b)) {
			return a.Num == b.Num
		}
		return false
	}
	switch a.Type {
	case "string":
		return a.Str == b.Str
	case "number":
		return a.Num == b.Num
	case "bool":
		return a.Bool == b.Bool
	}
	return false
}

func valStr(v Value) string {
	if v.IsNull {
		return "null"
	}
	switch v.Type {
	case "string":
		return v.Str
	case "number":
		if v.Num == float64(int64(v.Num)) && v.Num < 1e15 && v.Num > -1e15 {
			return fmt.Sprintf("%d", int64(v.Num))
		}
		return fmt.Sprintf("%g", v.Num)
	case "bool":
		if v.Bool {
			return "true"
		}
		return "false"
	case "function":
		return "<fn>"
	case "array":
		parts := make([]string, len(v.Array))
		for i, el := range v.Array {
			parts[i] = valStr(el)
		}
		return "[" + strings.Join(parts, ", ") + "]"
	case "object":
		return "<object>"
	}
	return fmt.Sprintf("<%s>", v.Type)
}
