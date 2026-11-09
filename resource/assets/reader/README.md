# Reader extraction dependencies

- `readability.js`: Mozilla Readability **0.6.0**, commit
  `04fd32f72b448c12b02ba6c40928b67e510bac49`, from
  https://github.com/mozilla/readability. Apache-2.0; see `readability-LICENSE.md`.
  SHA-256: `34dcab3d0832d0019f02990eed6b6124e029e8c32b9f0c6f2550544ff8dff174`.
- `purify.js`: DOMPurify **3.4.16**, upstream `dist/purify.min.js` from
  https://github.com/cure53/DOMPurify/tree/3.4.16. Used under Apache-2.0;
  see `purify-LICENSE` and the source header.
  SHA-256: `2c90a9b46d6463f26038a29b686e82bc91de01fdac9d5229e7cfe3b360134ea2`.
- `extract.js`: Fleet Reader's adapter. The upstream libraries are unchanged. Update the pinned
  versions, hashes, licenses, and browser regression tests together when upgrading.

The app accesses these files through generated `res::assets::reader` constants. Publication
HTML is data passed to the adapter, never executable script or a bundled UI translation.

User-installed scripts are separate from these vendored libraries. The adapter accepts an
optional synchronous extraction hook; its input document and returned HTML are sanitized
just as for Mozilla extraction. See the main README for the hook API and site matching.
