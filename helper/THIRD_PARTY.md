# Libraries

The helper is built from these crates, fetched by cargo; none of their code is in this repository. If you pass on a
built `micropanel`, include their licence texts (`tools/third_party_licenses.py` collects them; the release has them).

| Crate | Version | Licence | Used on |
|---|---|---|---|
| [ascii](https://github.com/tomprogrammer/rust-ascii) | 1.1.0 | Apache-2.0 OR MIT | all |
| [basic-udev](https://github.com/xobs/basic-udev) | 0.1.3 | MIT OR Apache-2.0 | Linux |
| [bitflags](https://github.com/bitflags/bitflags) | 2.13.2 | MIT OR Apache-2.0 | all |
| [block-buffer](https://github.com/RustCrypto/utils) | 0.10.4 | MIT OR Apache-2.0 | all |
| [byteorder](https://github.com/BurntSushi/byteorder) | 1.5.0 | Unlicense OR MIT | all |
| [bytes](https://github.com/tokio-rs/bytes) | 1.12.2 | MIT | all |
| [cc](https://github.com/rust-lang/cc-rs) | 1.11.2 | MIT OR Apache-2.0 | all |
| [cfg-if](https://github.com/rust-lang/cfg-if) | 1.0.5 | MIT OR Apache-2.0 | all |
| [cfg_aliases](https://github.com/katharostech/cfg_aliases) | 0.2.2 | MIT | Linux |
| [chunked_transfer](https://github.com/frewsxcv/rust-chunked-transfer) | 1.5.0 | MIT OR Apache-2.0 | all |
| [cpufeatures](https://github.com/RustCrypto/utils) | 0.2.17 | MIT OR Apache-2.0 | all |
| [crossbeam-channel](https://github.com/crossbeam-rs/crossbeam) | 0.5.17 | MIT OR Apache-2.0 | Windows |
| [crossbeam-utils](https://github.com/crossbeam-rs/crossbeam) | 0.8.23 | MIT OR Apache-2.0 | Windows |
| [crypto-common](https://github.com/RustCrypto/traits) | 0.1.7 | MIT OR Apache-2.0 | all |
| [data-encoding](https://github.com/ia0/data-encoding) | 2.11.1 | MIT | all |
| [digest](https://github.com/RustCrypto/traits) | 0.10.7 | MIT OR Apache-2.0 | all |
| [dpi](https://github.com/rust-windowing/winit) | 0.1.2 | Apache-2.0 AND MIT | Windows |
| [equivalent](https://github.com/indexmap-rs/equivalent) | 1.0.3 | Apache-2.0 OR MIT | all |
| [find-msvc-tools](https://github.com/rust-lang/cc-rs) | 0.1.15 | MIT OR Apache-2.0 | all |
| [generic-array](https://github.com/fizyk20/generic-array.git) | 0.14.7 | MIT | all |
| [getrandom](https://github.com/rust-random/getrandom) | 0.2.17 | MIT OR Apache-2.0 | all |
| [hashbrown](https://github.com/rust-lang/hashbrown) | 0.17.1 | MIT OR Apache-2.0 | all |
| [hidapi](https://github.com/ruabmbua/hidapi-rs) | 2.6.7 | MIT | all |
| [http](https://github.com/hyperium/http) | 1.5.0 | MIT OR Apache-2.0 | all |
| [httparse](https://github.com/seanmonstar/httparse) | 1.10.1 | MIT OR Apache-2.0 | all |
| [httpdate](https://github.com/pyfisch/httpdate) | 1.0.3 | MIT OR Apache-2.0 | all |
| [indexmap](https://github.com/indexmap-rs/indexmap) | 2.14.2 | Apache-2.0 OR MIT | all |
| [itoa](https://github.com/dtolnay/itoa) | 1.0.19 | MIT OR Apache-2.0 | all |
| [keyboard-types](https://github.com/rust-windowing/keyboard-types) | 0.8.3 | MIT OR Apache-2.0 | Windows |
| [libc](https://github.com/rust-lang/libc) | 0.2.191 | MIT OR Apache-2.0 | all |
| [log](https://github.com/rust-lang/log) | 0.4.35 | MIT OR Apache-2.0 | all |
| [memchr](https://github.com/BurntSushi/memchr) | 2.8.3 | Unlicense OR MIT | all |
| [muda](https://github.com/tauri-apps/muda) | 0.21.2 | Apache-2.0 OR MIT | Windows |
| [nix](https://github.com/nix-rust/nix) | 0.30.1 | MIT | Linux |
| [once_cell](https://github.com/matklad/once_cell) | 1.21.4 | MIT OR Apache-2.0 | Windows |
| [pkg-config](https://github.com/rust-lang/pkg-config-rs) | 0.3.34 | MIT OR Apache-2.0 | all |
| [ppv-lite86](https://github.com/cryptocorrosion/cryptocorrosion) | 0.2.21 | MIT OR Apache-2.0 | all |
| [proc-macro2](https://github.com/dtolnay/proc-macro2) | 1.0.107 | MIT OR Apache-2.0 | all |
| [quote](https://github.com/dtolnay/quote) | 1.0.47 | MIT OR Apache-2.0 | all |
| [rand](https://github.com/rust-random/rand) | 0.8.8 | MIT OR Apache-2.0 | all |
| [rand_chacha](https://github.com/rust-random/rand) | 0.3.1 | MIT OR Apache-2.0 | all |
| [rand_core](https://github.com/rust-random/rand) | 0.6.4 | MIT OR Apache-2.0 | all |
| [serde](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | all |
| [serde_core](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | all |
| [serde_derive](https://github.com/serde-rs/serde) | 1.0.229 | MIT OR Apache-2.0 | all |
| [serde_json](https://github.com/serde-rs/json) | 1.0.154 | MIT OR Apache-2.0 | all |
| [serde_spanned](https://github.com/toml-rs/toml) | 1.1.3 | MIT OR Apache-2.0 | all |
| [sha1](https://github.com/RustCrypto/hashes) | 0.10.7 | MIT OR Apache-2.0 | all |
| [shlex](https://github.com/comex/rust-shlex) | 2.0.1 | MIT OR Apache-2.0 | all |
| [syn](https://github.com/dtolnay/syn) | 2.0.119 | MIT OR Apache-2.0 | all |
| [syn](https://github.com/dtolnay/syn) | 3.0.10 | MIT OR Apache-2.0 | all |
| [thiserror](https://github.com/dtolnay/thiserror) | 1.0.69 | MIT OR Apache-2.0 | all |
| [thiserror](https://github.com/dtolnay/thiserror) | 2.0.21 | MIT OR Apache-2.0 | Windows |
| [thiserror-impl](https://github.com/dtolnay/thiserror) | 1.0.69 | MIT OR Apache-2.0 | all |
| [thiserror-impl](https://github.com/dtolnay/thiserror) | 2.0.21 | MIT OR Apache-2.0 | Windows |
| [tiny_http](https://github.com/tiny-http/tiny-http) | 0.12.0 | MIT OR Apache-2.0 | all |
| [toml](https://github.com/toml-rs/toml) | 1.2.1+spec-1.1.0 | MIT OR Apache-2.0 | all |
| [toml_datetime](https://github.com/toml-rs/toml) | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | all |
| [toml_edit](https://github.com/toml-rs/toml) | 0.25.19+spec-1.1.0 | MIT OR Apache-2.0 | all |
| [toml_parser](https://github.com/toml-rs/toml) | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | all |
| [toml_writer](https://github.com/toml-rs/toml) | 1.2.0+spec-1.1.0 | MIT OR Apache-2.0 | all |
| [tray-icon](https://github.com/tauri-apps/tray-icon) | 0.26.1 | MIT OR Apache-2.0 | Windows |
| [tungstenite](https://github.com/snapview/tungstenite-rs) | 0.24.0 | MIT OR Apache-2.0 | all |
| [typenum](https://github.com/paholg/typenum) | 1.20.1 | MIT OR Apache-2.0 | all |
| [unicode-ident](https://github.com/dtolnay/unicode-ident) | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | all |
| [utf-8](https://github.com/SimonSapin/rust-utf8) | 0.7.6 | MIT OR Apache-2.0 | all |
| [version_check](https://github.com/SergioBenitez/version_check) | 0.9.5 | MIT/Apache-2.0 | all |
| [windows-link](https://github.com/microsoft/windows-rs) | 0.2.1 | MIT OR Apache-2.0 | Windows |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.61.2 | MIT OR Apache-2.0 | Windows |
| [winnow](https://github.com/winnow-rs/winnow) | 1.0.5 | MIT | all |
| [zerocopy](https://github.com/google/zerocopy) | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | all |
| [zmij](https://github.com/dtolnay/zmij) | 1.0.23 | MIT | all |
