# src/common/process

Platform halves of the argv-list subprocess runner in `../process.rs`: `unix.rs`
captures output over nonblocking pipes under one deadline and byte ceiling.
Every subprocess in the crate goes through this runner; there is no shell-string
path.
