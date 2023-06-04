# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- next-header -->

## [Unreleased] <!-- release-date -->

## [0.4.0] - 2023-06-04

### Added 

- Add Credentials section to `README.md`.
- Add `cargo-release` configuration.

### Changed

- Replace [`html-escape`](https://crates.io/crates/html-escape) by [`htmlize`](https://crates.io/crates/htmlize), capable of decoding non-numeric HTML entities.

### Fixed

- Change `maybe_join` to propagate future output result. ([#3](https://github.com/LeoniePhiline/elopage-dl/issues/3))

### Removed

_(none)_

## [0.3.0] - 2023-05-10

### Added 

- Add support for YouTube embeds, next to the existing support for Vimeo embeds and the built-in wistia support. Needs [`yt-dlp`](https://github.com/yt-dlp/yt-dlp).

### Changed

- All lesson content is now discovered eagerly, then downloads are performed in parallel, if a `--parallel` value greater than `1` (default) is provided. (Closes [#2](https://github.com/LeoniePhiline/elopage-dl/issues/2) with an alternative approach.)
- Change lessons list to tree conversion algorithm to support all cases of nested categories and root lessons. (Fixes [#1](https://github.com/LeoniePhiline/elopage-dl/issues/1).)
- Hoisting root lessons into empty categories, where root categories are used as separators rather than as containers.

### Fixed

- Asset names might contain HTML entities despite being served in JSON format. These HTML entities are now decoded.
- Directory names are now whitespace-trimmed.
- Asterisks in directory names are replaced by dashes.

## [0.2.0] - 2023-05-03

### Added

- Now using [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) to download vimeo embeds from content blocks if elopage built-in wistia support is not used.
- Add `--parallel` option to offline-cache assets of multiple lessons at the same time.
- Add `tracing` support.
  Use `RUST_LOG=elopage_dl=debug` instead of `-vvv` to read debug output produced by the helper while skipping debug output produced by its dependencies, such as the `hyper` HTTP library.

### Changed

- Reword parts of `README.md` to be easier to understand and follow.
- Now using `Id` type alias consistently.
- Document new features.

## [0.1.0] - 2023-05-03

### Added

- Initial implementation.

<!-- next-url -->
[Unreleased]: https://github.com/LeoniePhiline/elopage-dl/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/LeoniePhiline/elopage-dl/compare/0.3.0...v0.4.0
[0.3.0]: https://github.com/LeoniePhiline/elopage-dl/releases/tag/0.3.0
[0.2.0]: https://github.com/LeoniePhiline/elopage-dl/releases/tag/0.2.0
[0.1.0]: https://github.com/LeoniePhiline/elopage-dl/releases/tag/0.1.0

