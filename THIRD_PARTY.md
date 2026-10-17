# Dependencies and protocol references

Online authentication and API calls use `tidalrs` 0.6.2 (MIT):
https://github.com/phayes/tidalrs . Its license is shipped in the Cargo registry
package and remains applicable to that dependency.

The public default device-client identifiers were verified against these projects:

- https://github.com/Nicolas1912i/TidalCLI/blob/main/src/config.rs (MIT)
- https://github.com/EbbLabs/python-tidal/blob/master/tidalapi/session.py (LGPL-3.0)

The client can override them with `TIDAL_CLIENT_ID` and `TIDAL_CLIENT_SECRET`.
No user tokens, passwords or private session data are included in this project.
The new application code was written independently; no reference implementation
source files were incorporated.
