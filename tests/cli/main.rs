//! End-to-end tests: the binary is driven exactly like a user would.
//!
//! One module per command, plus `support` (the fixture and its helpers) and
//! `formats` (one case per adapter). Each module stands on its own; a new test
//! goes into the module of the command it exercises.

mod add;
mod cd;
mod delete;
mod edit;
mod formats;
mod git;
mod harvest;
mod init;
mod output;
mod paths;
mod render;
mod rules;
mod status;
mod support;
mod update;
