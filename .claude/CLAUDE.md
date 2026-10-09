# Notes for working on this repository

- Everything in the repository is in English: code, comments, identifiers,
  UI text, error messages, docs, commit messages and test data. No Russian
  (or other Cyrillic) text anywhere, even in examples or tests. Conversation
  with the user can be in Russian; the repository cannot.
- Panic-free Rust: no `unwrap`/`expect`/`panic!`/indexing; the workspace
  clippy lints enforce this. Use `?` and the crate's own error types.
- Commit after every stage; keep `ROADMAP.md` up to date.
