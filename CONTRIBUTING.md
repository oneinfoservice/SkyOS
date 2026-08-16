# Contributing to SARGA OS

Thank you for your interest in contributing to **SARGA OS**, the userspace environment of SARGA OS. All contributions are welcome — code, documentation, testing, bug reports, feature requests, and ideas.

This project is governed by the **SKYIOUS Software License (SSL)**. By contributing, you agree that your contributions will be licensed under the same terms.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Ways to Contribute](#ways-to-contribute)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Build System](#build-system)
- [Commit Messages](#commit-messages)
- [Pull Request Process](#pull-request-process)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Documentation](#documentation)
- [Reporting Issues](#reporting-issues)
- [Feature Requests](#feature-requests)
- [Community](#community)
- [License](#license)

---

## Code of Conduct

We are committed to providing a welcoming, inclusive, and harassment-free experience for everyone, regardless of:

- Age, body size, disability, ethnicity, gender identity and expression
- Level of experience, nationality, personal appearance, race, religion
- Sexual identity and orientation, or any other dimension of diversity

### Expected Behavior

- Be respectful and considerate in all interactions
- Use welcoming and inclusive language
- Accept constructive criticism gracefully
- Focus on what is best for the community and the project
- Show empathy towards other community members

### Unacceptable Behavior

- Harassment, intimidation, or discrimination in any form
- Trolling, insulting/derogatory comments, and personal or political attacks
- Publishing others' private information without explicit permission
- Any other conduct which could reasonably be considered inappropriate

### Enforcement

Project maintainers are responsible for clarifying and enforcing these standards. Violations may be reported to the project team and will be addressed promptly. Consequences may range from a warning to temporary or permanent ban from the project.

---

## Ways to Contribute

You don't need to write code to contribute. Here are many ways to help:

| Area | How to Contribute |
|------|-------------------|
| **Report bugs** | Open an issue with clear steps to reproduce |
| **Suggest features** | Open an issue describing the feature and its use case |
| **Write documentation** | Improve README, doc comments, guides, and examples |
| **Write tests** | Add unit tests, integration tests, or edge-case coverage |
| **Fix bugs** | Pick an open issue and submit a pull request |
| **Add coreutils** | Port or implement a Unix utility missing from the repo |
| **Improve GUI apps** | Enhance skyedit, skyfiles, skyview, calculator |
| **Build tooling** | Improve build scripts, CI, dev experience |
| **Code review** | Review open pull requests |
| **Answer questions** | Help others in issues and discussions |
| **Spread the word** | Star the repo, write about the project, tell friends |

---

## Getting Started

### Prerequisites

- **Rust** (nightly channel)
- **Git**
- Basic familiarity with Rust and operating system concepts

### Setup

```bash
# Clone the repository
git clone https://github.com/SKYIOUS/SARGA-OS.git
cd SARGA-OS

# Ensure you have nightly Rust
rustup default nightly

# Install required components
rustup component add rust-src
rustup component add llvm-tools-preview

# Build the project
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release
```

### Finding Your First Contribution

- Look for issues labeled `good first issue` or `help wanted`
- Check the `plan.md` for upcoming work items
- Look at existing coreutils for patterns if you want to add a new utility
- Browse the `libsarga` library to understand the syscall wrappers

---

## Development Workflow

### Branching

- `master` — the main development branch, always buildable
- For changes, create a new branch from `master`:
  ```bash
  git checkout -b feature/my-feature
  ```

### Development Loop

```powershell
# 1. Make your changes
# 2. Build and test
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json

# 3. Build a specific component
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json -p coreutils

# 4. Run the full dev loop (builds userspace + kernel + QEMU)
.\scripts\dev_loop.ps1
```

### Keeping Your Fork Updated

```bash
git remote add upstream https://github.com/SKYIOUS/SARGA-OS.git
git fetch upstream
git rebase upstream/master
```

---

## Build System

SARGA OS uses a workspace Cargo.toml with 19 member crates. The custom target `x86_64-sarga.json` enables the `no_std` environment.

### Targets

| Target | Architecture | Description |
|--------|-------------|-------------|
| `x86_64-sarga.json` | x86_64 | Primary userspace target |
| `aarch64-sarga.json` | aarch64 | ARM64 userspace target |

### Profiles

| Profile | Opt Level | LTO | Panic |
|---------|-----------|-----|-------|
| Debug | 0 | No | abort |
| Release | 3 | Fat LTO | abort |

### Key Build Commands

```bash
cargo build                     # Debug build for host (host builds work; see libsarga's cargo test)
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release    # Release build for SARGA target
cargo build -p <crate>          # Build a single crate
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release -p ade    # Build just the desktop env

# Full build (cross-platform)
python build_disk.py

# Kernel-only build (faster for kernel development)
python build_disk.py --kernel-only

# Userspace-only build
python build_disk.py --userspace-only
```

---

## Commit Messages

We follow the **Conventional Commits** specification. Each commit message must be in the format:

```
<type>(<scope>): <short description>

<body (optional)>

<footer (optional)>
```

### Types

| Type | Usage |
|------|-------|
| `feat` | A new feature |
| `fix` | A bug fix |
| `docs` | Documentation only changes |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `test` | Adding or modifying tests |
| `chore` | Build process, CI, tooling changes |
| `style` | Formatting, missing semicolons, etc. (no code change) |
| `perf` | A performance improvement |

### Scopes

| Scope | Area |
|-------|------|
| `lib` | libsarga library |
| `shell` | sash shell |
| `init` | Init process |
| `coreutils` | Core utilities |
| `gui` | GUI applications and widget toolkit |
| `net` | Networking tools |
| `pkg` | Package manager (spkg) |
| `build` | Build system, scripts |
| `target` | Target specifications, linker scripts |
| `ci` | CI configuration |

### Examples

```
feat(coreutils): implement `wc` utility with -l, -w, -c flags

docs(lib): add doc comments to all public syscall wrappers

fix(shell): handle empty PATH correctly in command resolution

refactor(gui): extract common widget rendering into shared module
```

---

## Pull Request Process

1. **Before starting**, check if an issue exists for your change. If not, open one to discuss.

2. **Fork** the repository and create a feature branch.

3. **Write your code** following the coding standards below.

4. **Build and test** your changes locally:
   ```bash
   cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release
   ```

5. **Commit** your changes with a descriptive commit message.

6. **Push** to your fork and submit a pull request against `master`.

7. **Describe your changes** in the PR. Include:
   - What the change does
   - Why it's needed
   - How it was tested
   - Any breaking changes or migration steps

8. **Respond to review feedback** and make requested changes.

9. **After approval**, a maintainer will merge your PR.

### PR Checklist

- [ ] Code compiles without warnings
- [ ] Follows coding style (rustfmt)
- [ ] Includes doc comments for public API
- [ ] Documented unsafe blocks with `// SAFETY:` reasons
- [ ] Includes tests where applicable
- [ ] Commit messages follow Conventional Commits
- [ ] PR description clearly explains the change

---

## Coding Standards

### General

- Use Rust 2021 edition
- Code must compile with `#![deny(warnings)]` where applicable
- Run `cargo fmt` before committing
- No `std` — all crates are `no_std` with `extern crate alloc;`

### Documentation

- Add `///` doc comments to all public functions, structs, enums, and traits
- Add `//!` module-level doc comments to every new file
- Include examples in doc comments where helpful
- Document error conditions and panics

### Safety

- Avoid `unwrap()` and `expect()` in library code. Return `Result` instead.
- Justify every `unsafe` block with a `// SAFETY:` comment explaining why the operation is sound
- Prefer safe abstractions over raw pointer manipulation
- Validate user input from syscalls

### Naming

- Follow standard Rust naming conventions:
  - `snake_case` for functions, methods, variables, modules
  - `UpperCamelCase` for types, enums, traits
  - `SCREAMING_SNAKE_CASE` for constants and statics
  - Descriptive names over short ones

### libsarga Patterns

Syscall wrappers follow a consistent pattern:

```rust
/// Opens a file at the given path with the specified flags.
///
/// Returns a file descriptor on success, or an error code.
pub fn open(path: &str, flags: u32) -> Result<usize, Error> {
    let path_bytes = path.as_bytes();
    let result = syscall!(SYS_OPEN, path_bytes.as_ptr(), path_bytes.len(), flags);
    if result < 0 { Err(Error::from(result)) } else { Ok(result) }
}
```

---

## Testing

### Unit Tests

Tests live in the same file as the code they test, inside `#[cfg(test)]` modules:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flags() {
        assert_eq!(parse_flags("rw"), Some(Flags::Read | Flags::Write));
    }
}
```

### Build Verification

Always ensure your changes compile:

```bash
# Build everything
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release

# Build just the affected crate (faster)
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release -p libsarga
cargo build -Zbuild-std=core,alloc --target x86_64-sarga.json --release -p sash

# Host unit tests for pure logic in libsarga (no QEMU, no kernel)
cargo test -p libsarga
```

### Integration Testing

Integration tests require booting in QEMU with the SARGA kernel:

```bash
# Run the full dev loop
.\scripts\dev_loop.ps1
```

---

## Documentation

Good documentation is essential. Please contribute to:

- **Doc comments** on all public API
- **README updates** for new features
- **The docs/ directory** for deeper guides and references
- **Code examples** in doc comments
- **Error documentation** — what can fail and why

---

## Reporting Issues

When reporting a bug, please include:

- **Clear title** describing the issue
- **Steps to reproduce** — what commands, inputs, or actions lead to the bug
- **Expected behavior** — what should happen
- **Actual behavior** — what actually happens
- **Environment** — build target, QEMU or hardware, any relevant config
- **Logs or screenshots** if applicable

```markdown
Title: ls crashes when listing directory with special characters

Steps to reproduce:
1. Create a file named `test$file.txt`
2. Run `ls`

Expected: The file is listed
Actual: Shell reports "Segmentation fault"

Environment: x86_64-sarga, QEMU, commit abc123
```

---

## Feature Requests

Feature requests are welcome. Please include:

- **What** — describe the feature clearly
- **Why** — what problem does it solve, what use case does it enable
- **How** — any ideas on implementation (optional)
- **Prior art** — similar features in other systems (optional)

---

## Community

- **Issues**: Use GitHub issues for bugs and feature requests
- **Pull Requests**: For code contributions
- **Discussions**: Use GitHub discussions for questions and ideas

We strive to:
- Review PRs within 7 days
- Respond to issues within 3 days
- Be respectful and constructive in all feedback

---

## License

By contributing to SARGA OS, you agree that your contributions will be licensed under the **SKYIOUS Software License (SSL) v1.0**. See the [LICENSE](LICENSE) file for details.

This project and its contributors operate under the principle that **code contributions are irrevocable** — once submitted and accepted, they become part of the project under the project's license.
