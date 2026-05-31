<h1 align="center">ignity-extensions</h1>

<p align="center">
  <i align="center">High-performance extensions for ignity containers.</i>
  <br>
  <i align="center">Statically compiled Rust binaries that replace performance-sensitive execlineb scripts.</i>
</p>

<h4 align="center">
  <a href="https://github.com/techcode-io/ignity-extensions/actions/workflows/ci.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/techcode-io/ignity-extensions/ci.yml?branch=main&label=ci&style=flat-square" alt="continuous integration" style="height: 20px;">
  </a>
  <a href="https://github.com/techcode-io/ignity-extensions/graphs/contributors">
    <img src="https://img.shields.io/github/contributors-anon/techcode-io/ignity-extensions?color=yellow&style=flat-square" alt="contributors" style="height: 20px;">
  </a>
  <a href="https://opensource.org/licenses/MIT">
    <img src="https://img.shields.io/badge/MIT-blue.svg?style=flat-square&label=license" alt="license" style="height: 20px;">
  </a>
  <br>
</h4>

- [Source](https://github.com/techcode-io/ignity-extensions)
- [Issues](https://github.com/techcode-io/ignity-extensions/issues)
- [Contact](mailto:adrien.mannocci@gmail.com)
- [Maintained by techcode.io](https://techcode.io)

## :package: Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) stable toolchain.
- [musl-tools](https://packages.debian.org/musl-tools) for building static binaries on Linux.

## :sparkles: Features

- Fully static binaries — run on any Linux (glibc, musl/Alpine, distroless, scratch)
- Zero process-spawn overhead — all operations are in-process syscalls
- Drop-in replacements for the original execlineb scripts
- Single package containing all extensions (`.deb` and `.rpm`)

## :dart: Motivation

- ignity's built-in scripts fork `find`, `xargs`, `chown`, and `chmod` for every path spec.
- This overhead compounds on large container image trees with many files.
- Rust extensions perform the same work with a single directory traversal and direct syscalls.

## :hammer: Workflow

### Setup

The following steps will ensure your project is cloned properly.

1. `git clone https://github.com/techcode-io/ignity-extensions`
2. `cd ignity-extensions`

### Test

- To test you have to use the workflow script.

```bash
cargo test
```

- It will test project code with the current environment.

### Build

- To build a release binary you have to use the workflow script.

```bash
cargo build --release
```

- It will create an optimised binary for the current platform.

### Build (static)

- To build a fully static binary for any Linux target.

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

- The binary at `target/x86_64-unknown-linux-musl/release/fix-perms` has zero runtime dependencies.

## 📖 Usage

### Available extensions

| Crate | Replaces | Purpose |
| ----- | -------- | ------- |
| [`fix-perms`](crates/fix-perms/) | `src/usr/bin/fix-perms` | Apply ownership & permission specs to filesystem paths |

### How it works

- Extensions are statically compiled Rust binaries.
- They are drop-in replacements: same stdin format, same environment variables, same exit codes.
- During ignity's Stage 2, every file under `/etc/ignity/perms/` is concatenated and piped to `fix-perms`.
- The Rust binary replaces the original execlineb script that forked `find | xargs chown` and `find | xargs chmod` per path.

### How to integrate ignity-extensions in your project

- You can copy the static binary directly into your image at the path ignity expects.

```Dockerfile
COPY --from=build /usr/local/bin/fix-perms /usr/bin/fix-perms
```

- Or install the `.deb` / `.rpm` package during your image build.

```Dockerfile
COPY ignity-extensions_*.deb /tmp/
RUN dpkg -i /tmp/ignity-extensions_*.deb && rm /tmp/ignity-extensions_*.deb
```

### How to fix ownership & permissions

- Just like with ignity, place spec files in `/etc/ignity/perms/`.
- The pattern format followed by `fix-perms` files:

```text
path account fmode dmode
/var/lib/mysql 1000:1000 0600 0700
```

- `path`: File or dir path, processed recursively.
- `account`: Target account `uid:gid`.
- `fmode`: Target file mode. For example, `0644`.
- `dmode`: Target dir/folder mode. For example, `0755`.

- You can use variables `{{USERMAP_UID}}` and `{{USERMAP_GID}}` in those files.
- They will be replaced at runtime or build time based on case.
- Blank lines and lines starting with `#` are ignored.

### Available environment variables

| Variable | Description | Value (Default) |
| -------- | ----------- | --------------- |
| `USERMAP_UID` | User uid substituted for `{{USERMAP_UID}}` placeholders | `0` |
| `USERMAP_GID` | User gid substituted for `{{USERMAP_GID}}` placeholders | `0` |

- If either variable is set to a non-integer value, `fix-perms` emits a warning and falls back to `0`.

### How to create a versioned release

- To release a new version, create and push a tag.

```bash
git tag v0.1.0
git push --tags
```

- The CI pipeline derives the package version from the tag (`v0.1.0` → `1.0.0`).
- It will build the static binary, then produce a `.deb` and `.rpm` package uploaded as CI artifacts.

### How to add a new extension

- Create a new crate under `crates/`.

```bash
cargo new --bin crates/<name>
```

- Add it to the workspace in the root `Cargo.toml`.

```toml
members = ["crates/fix-perms", "crates/<name>"]
```

- Add an entry to `nfpm.yaml` so it is included in the packages.

```yaml
contents:
  - src: target/x86_64-unknown-linux-musl/release/<name>
    dst: /usr/local/bin/<name>
    file_info:
      mode: 0755
```

- It is convenient to prefix every script in `perms`, `init` and `finalize` by a number (two chars) to ensure execution order.
- A common pattern is to dedicate 10 numbers per image layer to allow logic evolution.

## :heart: Contributing

If you find this project useful here's how you can help, please click the :eye: **Watch** button to avoid missing
notifications about new versions, and give it a :star2: **GitHub Star**!

You can also contribute by:

- Sending a [Pull Request](https://github.com/techcode-io/ignity-extensions/pulls) with your awesome new features and bug fixes.
- Be part of the community and help resolve [Issues](https://github.com/techcode-io/ignity-extensions/issues).

## 🧾 License

The `ignity-extensions` project is free and open-source software licensed under the MIT license.
