# Security policy

Please report vulnerabilities privately through [GitHub Security Advisories](https://github.com/fritz-fritz/am5-spd-diag/security/advisories/new) or email [code@fritztech.net](mailto:code@fritztech.net). Do not open a public issue for a report that includes a working exploit.

This tool reads SPD5118 hubs over I²C. Snapshot, probe, and fix helpers run privileged. `fix` writes MR11 only when it already reads `0x08`; it does not rewrite EEPROM. Treat unexpected privilege prompts as untrusted.

Captures under `/var/log/am5-spd-diag/` are world-readable (board and DIMM serials included) and root-owned. Local users can inspect them; they cannot write that tree. `package` puts a copy in a tarball you own.

Vendor tickets from this tool include board and DIMM serials and omit system UUID and asset tags. Keep extra machine identifiers out of public reports when you can.

Packages come from the [Open Build Service](https://software.opensuse.org/download/package?package=am5-spd-diag&project=home:fritz-fritz). GitHub attached assets are archival copies of release packages. The Release workflow attests the source tarball and `SHA256SUMS` (the hashes of the attached files). OBS rpm/deb binaries are built and signed on OBS, not on GitHub Actions.

```bash
gh release verify vX.Y.Z
gh release verify-asset vX.Y.Z am5-spd-diag-X.Y.Z.tar.xz
gh attestation verify am5-spd-diag-X.Y.Z.tar.xz -R fritz-fritz/am5-spd-diag
gh attestation verify SHA256SUMS -R fritz-fritz/am5-spd-diag
```
