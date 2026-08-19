# Security policy

Please report vulnerabilities privately through [GitHub Security Advisories](https://github.com/fritz-fritz/am5-spd-diag/security/advisories/new) or email [code@fritztech.net](mailto:code@fritztech.net). Do not open a public issue for a report that includes a working exploit.

This tool reads SPD5118 hubs over I²C. Snapshot, probe, and fix helpers run privileged. `fix` writes MR11 only when it already reads `0x08`; it does not rewrite EEPROM. Treat unexpected privilege prompts as untrusted.

Vendor tickets from this tool include board and DIMM serials and omit system UUID and asset tags. Keep extra machine identifiers out of public reports when you can.
