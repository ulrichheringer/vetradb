# Foundation binary fixtures

Static hexadecimal examples for the [format-v1 candidate](../../specs/persistent-format-v1.md). Whitespace is insignificant; `bytes.fromhex()` yields the complete container. No fixture is an executable or a supported database file.

| File | Container |
| --- | --- |
| [superblock.hex](superblock.hex) | Initial redundant-superblock contents, 8192 bytes |
| [empty-leaf.hex](empty-leaf.hex) | Empty tree-2/PageID-2 leaf, 8192 bytes |
| [segment.hex](segment.hex) | Segment-0 lineage/header, 64 bytes |
| [row-envelope.hex](row-envelope.hex) | One row insertion, 163 bytes |
| [envelope-chunk.hex](envelope-chunk.hex) | Envelope fragment at LSN 64, 243 bytes |
| [commit.hex](commit.hex) | Matching commit at LSN 307, 88 bytes |

Run `python3 tools/verify_foundation.py` from the repository root. [Evidence and manual recovery traces](../../specs/foundation-evidence.md) describe exactly what is checked and what still requires implementation. Changes to these golden bytes require matching specification/version review.
