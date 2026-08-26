package interpreter

import (
	"fmt"
	"io"
	"os"
)

func RunSource(source string, output io.Writer) error {
	if output == nil {
		output = os.Stdout
	}
	lexer := NewLexer(source)
	tokens := lexer.Tokenize()
	parser := NewParser(tokens)
	prog := parser.Parse()
	interp := NewInterpreter()
	interp.SetOutput(output)

	defer func() {
		if r := recover(); r != nil {
			switch r.(type) {
			case returnSignal:
				fmt.Fprintln(os.Stderr, "error: return outside function")
			case breakSignal:
				fmt.Fprintln(os.Stderr, "error: break outside loop")
			case continueSignal:
				fmt.Fprintln(os.Stderr, "error: continue outside loop")
			default:
				panic(r)
			}
		}
	}()

	interp.Run(prog)
	return nil
}

func (interp *Interpreter) RunSource(source string) error {
	lexer := NewLexer(source)
	tokens := lexer.Tokenize()
	parser := NewParser(tokens)
	prog := parser.Parse()

	defer func() {
		if r := recover(); r != nil {
			switch r.(type) {
			case returnSignal:
				fmt.Fprintln(os.Stderr, "error: return outside function")
			case breakSignal:
				fmt.Fprintln(os.Stderr, "error: break outside loop")
			case continueSignal:
				fmt.Fprintln(os.Stderr, "error: continue outside loop")
			default:
				panic(r)
			}
		}
	}()

	interp.Run(prog)
	return nil
}

func RunFile(filename string) error {
	data, err := os.ReadFile(filename)
	if err != nil {
		return fmt.Errorf("error reading file: %w", err)
	}
	return RunSource(string(data), os.Stdout)
}
