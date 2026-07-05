# Bundled test font

`DejaVuSans.ttf` — [DejaVu Fonts](https://dejavu-fonts.github.io/), released under
the permissive DejaVu Fonts License (a superset of the Bitstream Vera Fonts
License). Free to use, embed, and redistribute; see
<https://dejavu-fonts.github.io/License.html>.

It is vendored **for tests only**: the export tests inject a `fontdb` containing
just this face so slide rendering is deterministic and CI needs no installed
system fonts. Production font resolution uses `export::system_fonts()`.
