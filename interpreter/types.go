package interpreter

import (
	"fmt"
	"os"
)

type BType string

const (
	TYPE_ANY    BType = "any"
	TYPE_INT    BType = "int"
	TYPE_FLOAT  BType = "float"
	TYPE_STRING BType = "string"
	TYPE_BOOL   BType = "bool"
	TYPE_ARRAY  BType = "array"
	TYPE_FN     BType = "fn"
	TYPE_BEL    BType = "bel"
	TYPE_STER   BType = "ster"
)

func isNumericType(t BType) bool {
	return t == TYPE_INT || t == TYPE_FLOAT || t == TYPE_BEL
}

func typeCompatible(declared, actual BType) bool {
	if declared == TYPE_ANY || actual == TYPE_ANY {
		return true
	}
	if declared == actual {
		return true
	}
	if isNumericType(declared) && isNumericType(actual) {
		return true
	}
	if declared == TYPE_STER && actual == TYPE_STRING {
		return true
	}
	if declared == TYPE_STRING && actual == TYPE_STER {
		return true
	}
	return false
}

func runtimeTypeOf(v Value) BType {
	if v.IsNull {
		return TYPE_ANY
	}
	switch v.Type {
	case "string":
		return TYPE_STRING
	case "number":
		return TYPE_INT
	case "bool":
		return TYPE_BOOL
	case "array":
		return TYPE_ARRAY
	case "function":
		return TYPE_FN
	}
	return TYPE_ANY
}

func typeMismatchError(expected BType, actual BType, line, col int, context string) {
	fmt.Fprintf(os.Stderr, "line %d:%d: type error: expected %s, got %s in %s\n",
		line, col, expected, actual, context)
	os.Exit(1)
}
