# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added 

_(none)_

### Changed

_(none)_

### Fixed

_(none)_

### Removed

_(none)_

## [0.2.0] - 2023-05-03

### Added

- Now using `yt-dlp` to download vimeo embeds from content blocks if elopage built-in wistia support is not used.
- Added `--parallel` option to offline-cache assets of multiple lessons at the same time.
- Added `tracing` support.
  Use `RUST_LOG=elopage_dl=debug` instead of `-vvv` to read debug output produced by the helper while skipping debug output produced by its dependencies, such as the `hyper` HTTP library.

### Changed

- Reworded parts of `README.md` to be easier to understand and follow.
- Now using `Id` type alias consistently.
- Documented new features.

## [0.1.0] - 2023-05-03

### Added

- Initial implementation.

[unreleased]: https://github.com/LeoniePhiline/showcase-dl/compare/0.2.0...HEAD
[0.2.0]: https://github.com/LeoniePhiline/elopage-dl/releases/tag/0.2.0
[0.1.0]: https://github.com/LeoniePhiline/elopage-dl/releases/tag/0.1.0

