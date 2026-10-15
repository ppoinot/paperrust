# Native libraries

`libpdfium.so` is downloaded at deploy (not committed):

https://github.com/bblanchon/pdfium-binaries

Paperrust rasterizes PDF pages with the `pdfium-render` crate bound to this
shared library (`PAPERRUST_PDFIUM`). Images are decoded with the `image` crate.
There is no PDF.js viewer and no poppler dependency on the CoreOS host.
