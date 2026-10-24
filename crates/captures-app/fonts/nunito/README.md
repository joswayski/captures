# Nunito 3.601 static fonts

Four **unmodified static TTFs** (regular, bold, italic and bold-italic) for the
Tauri and experimental native editors' `rounded` family (shown as Nunito). Source: [googlefonts/nunito at
`4be812cf4761b3ddc3b0ae894ef40ea21dcf6ff3`](https://github.com/googlefonts/nunito/tree/4be812cf4761b3ddc3b0ae894ef40ea21dcf6ff3/fonts/TTF).
The pinned historical revision includes these static faces; current upstream
does not. Its `OFL.txt` is included verbatim with the full 2014 Nunito Project
Authors copyright and SIL Open Font License 1.1. Keep both fonts' complete
notices with native resources and font-bearing saved drafts. Tauri's CSS references
these same files; Vite emits their complete notice as `licenses/Nunito-OFL.txt`
alongside the packaged UI assets.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| Nunito-Regular.ttf | 154,312 | `880ead818af373b0da8c2a3e493f7d0a9c1db149a67a867b04f0952c475d667a` |
| Nunito-Bold.ttf | 154,140 | `f922b74ed5cca7c8675d352fd5ed197e0f38b3ac4a427c15b7a3db039dc6294c` |
| Nunito-Italic.ttf | 158,696 | `2e19528ebd89ae7bc0442744206730319a03b40d550ac3711c49b691286e7e10` |
| Nunito-BoldItalic.ttf | 161,216 | `e34e5bc2560aa0cd5254d7b1c6433f151b6a8dd36fd6f9096527f99fa36496fb` |

Total new font payload: **628,364 bytes**. These bytes are bundled offline;
new text-bearing drafts pin all four files, while image-only drafts do not.
Existing native drafts retain their exact saved font map and bytes without automatic
migration or substitution. Tauri's former system-dependent `rounded` stack now uses
these bundled faces, including for existing Rounded labels. In the regular faces' cmap, Nunito covers 938 code
points versus Liberation Sans's 2,327. Both cover é, ñ, č, Ω, Ж and я; Nunito
lacks Greek λ (and 1,409 other Liberation Sans code points). Native sessions use
the existing bundled/platform glyph fallback, and Tauri uses browser fallback;
this is not universal Unicode coverage or identical fallback on every OS.
Do not alter/subset the
files under their immutable `nunito-rounded-3-601-*` draft asset IDs.
