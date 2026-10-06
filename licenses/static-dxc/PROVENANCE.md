# Bundled Windows x64 shader compiler notices

Windows x86-64 MSVC EffectCraft binaries statically link a **native C/C++ DXC
compiler**, through wgpu and the Rust `mach-dxcompiler-rs` wrapper. DXC is not a Rust
implementation. Other supported EffectCraft targets do not enable this dependency.

The wrapper is `0.1.6+2026.09.16-48d5a66.1`, source revision
`77b9a2f68c146d1cf45e52ed7e7ac2457be875d5`. Its downloaded native library is from the
immutable `DouglasDwyer/mach-dxcompiler` release `2026.09.16+48d5a66.1`. That release's
`build.zig` pins `hexops/DirectXShaderCompiler` source revision
`4190bb0c90d374c6b4d0b0f2c7b45b604eda24b6` (now hosted in `hexops-graveyard`).

The wrapper is MIT-licensed. The mach integration offers MIT or Apache-2.0;
the MIT option and its general license statement are reproduced here. DXC/LLVM
and Clang retain the **University of Illinois/NCSA Open Source License** and the
additional terms in their original notices. NCSA is a permissive BSD-style license;
it is not relabeled as MIT, Apache-2.0 or BSD in this distribution. Its binary
redistribution condition requires the copyright, conditions and disclaimer to
accompany the binary. The OpenBSD regex notice also requires documentation credits.
These accompanying texts retain those credits and conditions.

`DirectXShaderCompiler-LICENSE.TXT` includes a section explicitly covering build/test
dependencies only, including the autoconf `config.guess` license. Those tools are
not linked into the compiler by this static-library build. The full upstream file
is preserved rather than silently deleting sections. The native fork's general
license also distinguishes documentation/assets; EffectCraft does not bundle those
upstream media assets as part of this dependency.

## Exact notice sources

The files below preserve the upstream texts. SHA-256 values identify the original
downloaded bytes; Git checkout line-ending conversion may change local byte hashes.

| File | Original SHA-256 |
| --- | --- |
| mach-dxcompiler-rs-LICENSE.txt | `56feda35bbc6b1e1446d59b2235e0b311793675b98700c0851f022f67c698d0c` |
| mach-dxcompiler-LICENSE.txt | `d7227ff78132e0dfb706c41bbecf803cf3b3d274c93722ebcbf307b3272323fe` |
| mach-dxcompiler-LICENSE-MIT.txt | `84cf02ebb6a2d05bbc7f9a151e9887f190e050cc7f5743869eedc0a9ada26a7f` |
| DirectXShaderCompiler-LICENSE.TXT | `27a49e35d1da96eba18fba54bc882667ff0ff8c0254f16f2b6e165d605ba7df8` |
| DirectXShaderCompiler-ThirdPartyNotices.txt | `19512a5d0a015ef16d167272c164da49a604614a786206212a80ad486ed0be6d` |

Primary sources:

- [Wrapper LICENSE](https://github.com/DouglasDwyer/mach-dxcompiler-rs/blob/77b9a2f68c146d1cf45e52ed7e7ac2457be875d5/LICENSE).
- [Native integration LICENSE](https://github.com/DouglasDwyer/mach-dxcompiler/blob/2026.09.16%2B48d5a66.1/LICENSE) and [LICENSE-MIT](https://github.com/DouglasDwyer/mach-dxcompiler/blob/2026.09.16%2B48d5a66.1/LICENSE-MIT).
- [Pinned native build.zig](https://github.com/DouglasDwyer/mach-dxcompiler/blob/2026.09.16%2B48d5a66.1/build.zig).
- [DXC LICENSE.TXT](https://github.com/hexops-graveyard/DirectXShaderCompiler/blob/4190bb0c90d374c6b4d0b0f2c7b45b604eda24b6/LICENSE.TXT) and [ThirdPartyNotices.txt](https://github.com/hexops-graveyard/DirectXShaderCompiler/blob/4190bb0c90d374c6b4d0b0f2c7b45b604eda24b6/ThirdPartyNotices.txt).

The release's native archive hashes (checked by the wrapper before linking) are
`60bdfab56d50679ef071489432fd716e5c2c73c7ec08c33a29d0c910ddca1b04` for static CRT
and `2db15ab14e6eee9b9931d75d1690b4f7d81cad446fecccfe4e36e218dee25cce` for dynamic
CRT. These are library archives, not the Rust crate or EffectCraft executable.
