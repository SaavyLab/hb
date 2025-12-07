# Changelog

## [0.2.0] - 2025-12-07

### Changed
- **Breaking**: Switched from `rsa`/`sha2` crates to Web Crypto API for JWT signature verification
- Replaced `once_cell` with `std::sync::OnceLock` (stable since Rust 1.70)

### Removed
- `rsa` dependency
- `sha2` dependency  
- `once_cell` dependency

### Added
- `web-sys` dependency (with Crypto, SubtleCrypto, CryptoKey features)
- `wasm-bindgen` dependency
- `wasm-bindgen-futures` dependency

### Performance
- ~39% smaller bundle size overhead (~43 KiB vs ~71 KiB gzipped)
- Faster cold starts due to reduced wasm binary size
- Uses native platform crypto (hardware-accelerated where available)

## [0.1.0] - Initial Release

- Cloudflare Access JWT verification
- Axum extractor support
- Optional KV caching for JWKS
