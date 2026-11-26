# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0](https://github.com/srnnkls/meros/releases/tag/v0.1.0) - 2026-11-26

### Features

- Read targets and apply plans safely
- Plan ownership outcomes
- Handle YAML targets
- Edit JSON and JSONC targets in place
- Edit TOML targets in place
- Derive units from documents and validate declarations
- Parse taxis paths and hash values under meros-1
- Model the meros domain as types

### Refactor

- Align the crate with the Rust guide

### Documentation

- Add the MIT license
- Document meros in the README
- *(study)* YAML timestamps hash as strings
- *(study)* Note the inline re-implementations of key ownership
- Decide the ownership model for values in shared files

### Testing

- Cover AC-1 to AC-32 with scenarios

### Miscellaneous Tasks

- Run the loqui gate on Linux and macOS
- Release meros from git tags, not crates.io
- Release meros with the loqui rust-release blueprint
- Adopt the loqui Rust blueprint lint gate
- *(phora)* Add apm and agent-bot-machine as reference resources

