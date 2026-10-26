# Pinned Lucide SVG catalog

`icons.json` contains all 1,903 SVGs from Lucide **1.61.0**, including the
release's provenance and archive checksum. The upstream ISC/MIT notices are
preserved in `LICENSE`.

Cargo builds generate the name lookup directly from these bytes. They never
scan `target/`, inspect another crate's build artifacts, or download icons.
Names and short aliases retain the existing case/separator normalization.

Regenerate identically with:

```sh
python3 scripts/update_lucide.py
# Or use a previously downloaded, checksum-verified release archive:
python3 scripts/update_lucide.py /path/to/lucide-1.61.0.tar.gz
```

To upgrade, explicitly update the version, checksum, and expected count in the
script, regenerate, review the catalog changes, and run the icon tests.
