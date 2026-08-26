package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strings"

	"belsk2/interpreter"
)

func main() {
	if len(os.Args) > 1 {
		filename := os.Args[1]
		if err := interpreter.RunFile(filename); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		return
	}

	stat, _ := os.Stdin.Stat()
	if (stat.Mode() & os.ModeCharDevice) == 0 {
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		if err := interpreter.RunSource(string(data), os.Stdout); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		return
	}

	fmt.Println("belsk2 v0.2 (typed)")
	fmt.Println("type 'exit' to quit")
	fmt.Println()

	interp := interpreter.NewInterpreter()
	interp.SetOutput(os.Stdout)

	scanner := bufio.NewScanner(os.Stdin)
	for {
		fmt.Print("belsk2> ")
		if !scanner.Scan() {
			break
		}
		input := strings.TrimSpace(scanner.Text())
		if input == "exit" || input == "quit" {
			fmt.Println("bye!")
			break
		}
		if input == "" {
			continue
		}

		fullInput := input
		braceDepth := 0
		for _, ch := range fullInput {
			if ch == '{' {
				braceDepth++
			}
			if ch == '}' {
				braceDepth--
			}
		}
		needMore := !strings.HasSuffix(fullInput, ";") && !strings.HasSuffix(fullInput, "}") || braceDepth > 0
		for needMore {
			fmt.Print("... ")
			if !scanner.Scan() {
				break
			}
			line := strings.TrimSpace(scanner.Text())
			fullInput += "\n" + line
			for _, ch := range line {
				if ch == '{' {
					braceDepth++
				}
				if ch == '}' {
					braceDepth--
				}
			}
			needMore = (!strings.HasSuffix(line, ";") && !strings.HasSuffix(line, "}") || braceDepth > 0) && braceDepth >= 0
			if braceDepth <= 0 && (strings.HasSuffix(line, ";") || strings.HasSuffix(line, "}")) {
				needMore = false
			}
		}

		if err := interp.RunSource(fullInput); err != nil {
			fmt.Fprintln(os.Stderr, err)
		}
	}
}
