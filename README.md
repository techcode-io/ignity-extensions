<h1 align="center">ignity-utils-rs</h1>

<p align="center">
  <i align="center">High-performance Rust drop-in replacements for ignity-utils.</i>
  <br>
  <i align="center">Statically compiled Rust binaries that replace performance-sensitive execlineb scripts.</i>
</p>

<h4 align="center">
  <a href="https://github.com/techcode-io/ignity-utils-rs/actions/workflows/ci.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/techcode-io/ignity-utils-rs/ci.yml?branch=main&label=ci&style=flat-square" alt="continuous integration" style="height: 20px;">
  </a>
  <a href="https://github.com/techcode-io/ignity-utils-rs/graphs/contributors">
    <img src="https://img.shields.io/github/contributors-anon/techcode-io/ignity-utils-rs?color=yellow&style=flat-square" alt="contributors" style="height: 20px;">
  </a>
  <a href="https://opensource.org/licenses/MIT">
    <img src="https://img.shields.io/badge/MIT-blue.svg?style=flat-square&label=license" alt="license" style="height: 20px;">
  </a>
  <br>
</h4>

- [Source](https://github.com/techcode-io/ignity-utils-rs)
- [Issues](https://github.com/techcode-io/ignity-utils-rs/issues)
- [Contact](mailto:adrien.mannocci@gmail.com)
- [Maintained by techcode.io](https://techcode.io)

## :package: Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) stable toolchain.
- [musl-tools](https://packages.debian.org/musl-tools) for building static binaries on Linux.

## :sparkles: Features

- Fully static binaries — run on any Linux (glibc, musl/Alpine, distroless, scratch)
- Zero process-spawn overhead — all operations are in-process syscalls
- Drop-in replacements for the original execlineb scripts
- Single package replacing `ignity-utils` (`.deb` and `.rpm`)

## :dart: Motivation

- ignity's built-in scripts fork `find`, `xargs`, `chown`, and `chmod` for every path spec.
- This overhead compounds on large container image trees with many files.
- Rust utils perform the same work with a single directory traversal and direct syscalls.

## :hammer: Workflow

### Setup

The following steps will ensure your project is cloned properly.

1. `git clone https://github.com/techcode-io/ignity-utils-rs`
2. `cd ignity-utils-rs`

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
- Use `aarch64-unknown-linux-musl` as target to build for `arm64`.

### Package

- To build `.deb` and `.rpm` packages from a static binary, you need [nfpm](https://nfpm.goreleaser.com/).

```bash
cargo scripts build --target x86_64-unknown-linux-musl
```

- It will create packages in `dist/` with the version defined in `Cargo.toml`.

## 📖 Usage

### Available utils

| Crate | Replaces | Purpose |
| ----- | -------- | ------- |
| [`fix-perms`](crates/fix-perms/) | `src/usr/bin/fix-perms` | Apply ownership & permission specs to filesystem paths |

### How it works

- Utils are statically compiled Rust binaries.
- They are drop-in replacements: same stdin format, same environment variables, same exit codes.
- During ignity's Stage 2, every file under `/etc/ignity/perms/` is concatenated and piped to `fix-perms`.
- The Rust binary replaces the original execlineb script that forked `find | xargs chown` and `find | xargs chmod` per path.

### How to integrate ignity-utils-rs in your project

- You can copy the static binary directly into your image at the path ignity expects.

```Dockerfile
COPY --from=build /usr/local/bin/fix-perms /usr/bin/fix-perms
```

- Or install the `.deb` / `.rpm` package during your image build.

```Dockerfile
COPY ignity-utils-rs_*.deb /tmp/
RUN dpkg -i /tmp/ignity-utils-rs_*.deb && rm /tmp/ignity-utils-rs_*.deb
```

- The `ignity-utils-rs` package replaces ignity's execline `ignity-utils` package.
- Both provide the virtual `ignity-utils-impl` that `ignity-core` depends on, so installing `ignity-utils-rs` removes `ignity-utils` and installs at the same paths.

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

- The version lives in the root `Cargo.toml` (`[workspace.package] version`) and is used for the packages.
- Run the `Pre release` workflow with the version to release (e.g. `0.2.0`).
- It opens a PR bumping `Cargo.toml` and `Cargo.lock`, merge it.
- Run the `Release` workflow with the next iteration version (e.g. `0.3.0-dev`).
- It runs the CI, creates the `v0.2.0` tag and a GitHub release with `.deb` and `.rpm` packages for `amd64` and `arm64`.
- It then opens a PR bumping the version to the next iteration, merge it.
- The release steps are also available locally through `cargo scripts release:pre`, `cargo scripts release` and `cargo scripts release:post`.

### How to add a new util

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
  - src: target/${TARGET}/release/<name>
    dst: /usr/bin/<name>
    file_info:
      mode: 0755
```

- It is convenient to prefix every script in `perms`, `init` and `finalize` by a number (two chars) to ensure execution order.
- A common pattern is to dedicate 10 numbers per image layer to allow logic evolution.

## :heart: Contributing

If you find this project useful here's how you can help, please click the :eye: **Watch** button to avoid missing
notifications about new versions, and give it a :star2: **GitHub Star**!

You can also contribute by:

- Sending a [Pull Request](https://github.com/techcode-io/ignity-utils-rs/pulls) with your awesome new features and bug fixes.
- Be part of the community and help resolve [Issues](https://github.com/techcode-io/ignity-utils-rs/issues).

## 🧾 License

The `ignity-utils-rs` project is free and open-source software licensed under the MIT license.
