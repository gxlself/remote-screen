# Release signing

Releases use local, gitignored signing files by default. No configuration server
or sibling checkout is required. Never commit keystores, passwords, API keys, or
filled configuration files.

For Android, create `app/android/keystore.local.properties`:

```properties
MYAPP_UPLOAD_STORE_FILE=app/release.keystore
MYAPP_UPLOAD_KEY_ALIAS=release
MYAPP_UPLOAD_STORE_PASSWORD=<store password>
MYAPP_UPLOAD_KEY_PASSWORD=<key password>
```

Place your release keystore at the indicated path (relative to `app/android/`,
or absolute). `keystore.properties` is the existing fallback. To use another
properties file, pass `--keystore-properties <file>` to
`bash tools/release/android.sh`. Missing signing fields or keystores fail the
release; the script never substitutes a debug signature.

For iOS, create `app/ios/asc-api-key.properties`:

```properties
ASC_API_KEY_ID=<App Store Connect key ID>
ASC_API_ISSUER_ID=<App Store Connect issuer UUID>
```

Place its private key at `app/AuthKey_<KEY_ID>.p8` with permissions `600`. Pass
`--team-id <TEAM_ID>` or set `RS_IOS_TEAM_ID` for signed releases.
`bash tools/release/ios.sh --plan` prints the steps without reading secrets or
running build tools. `--dry-run` skips signing and upload; it still builds an
unsigned archive on macOS. It cannot be combined with `--upload`.

## Optional Nacos provider

Set `BUILD_SECRETS_PROVIDER=nacos` and supply all four routing coordinates:
`NACOS_SERVER`, `NACOS_NAMESPACE`, `NACOS_GROUP`, and `NACOS_DATA_ID`. Set both
`NACOS_USERNAME` and `NACOS_PASSWORD` when authentication is required; leave both
empty for anonymous access. There are no built-in server or routing defaults.

Alternatively, copy `app/scripts/nacos-build-secrets.env.example` to a private
file, fill it in, and set `NACOS_CONFIG_FILE=/absolute/path/to/file`. Only that
explicitly named file is read. Values are parsed as text, never executed as shell
code; environment variables override the file, including explicit empty values.
Supplying a Nacos coordinate or a named config file also selects Nacos unless
`BUILD_SECRETS_PROVIDER=local` is set. Incomplete configuration fails before any
network request. Set `BUILD_SECRETS_PROVIDER=local` or pass `--skip-secrets-pull`
to the release wrapper to use local files.

```sh
bash app/scripts/nacos-build-secrets.sh pull --platform android --require-android-signing
bash app/scripts/nacos-build-secrets.sh pull --platform ios
bash app/scripts/nacos-build-secrets.sh diff
bash app/scripts/nacos-build-secrets.sh publish --apply
```

The properties format is unchanged: Nacos uses the `MYAPP_UPLOAD_*` signing
fields, `ANDROID_RELEASE_KEYSTORE_BASE64` (or `ANDROID_KEYSTORE_BASE64`), and
`IOS_ASC_API_KEY_ID`, `IOS_ASC_API_ISSUER_ID`, `IOS_ASC_API_KEY_P8_BASE64`.
Pull restores the local files above. Existing optional push and authentication
integration fields remain supported. Network or HTTP failures can fall back to
existing local files; release wrappers still validate signing before building.

Self-tests for the release wrappers (the first three run on Linux; the Mac one needs macOS):

```sh
bash tools/release/secrets/test.sh
bash tools/release/ios/test.sh
bash tools/release/android/test.sh
bash tools/release/mac/test.sh
```

## GitHub Actions

`.github/workflows/release.yml` builds both apps on GitHub-hosted runners:
Android on `ubuntu-24.04`, iOS on `macos-26`. Start it from the Actions tab
(Run workflow, with a build number and an optional TestFlight upload), or push
a tag `v<version>` or `v<version>-b<N>` that matches `[workspace.package]
version` in `Cargo.toml`. A tag uploads the signed iOS build to TestFlight.
The APK, AAB, IPA, dSYMs and release summaries are kept as workflow artifacts.

Signing comes from repository secrets, named like the Nacos fields:

| Secret | Value |
|---|---|
| `ANDROID_RELEASE_KEYSTORE_BASE64` | `base64 < release.keystore` |
| `MYAPP_UPLOAD_KEY_ALIAS`, `MYAPP_UPLOAD_STORE_PASSWORD`, `MYAPP_UPLOAD_KEY_PASSWORD` | as in `keystore.local.properties` |
| `IOS_ASC_API_KEY_ID`, `IOS_ASC_API_ISSUER_ID` | as in `asc-api-key.properties` |
| `IOS_ASC_API_KEY_P8_BASE64` | `base64 < AuthKey_<KEY_ID>.p8` |
| `IOS_TEAM_ID` | the Apple Developer team ID |
| `IOS_SIGNING_CERT_P12_BASE64`, `IOS_SIGNING_CERT_PASSWORD` | optional: an exported Apple Development certificate and its password |

For example: `base64 < release.keystore | gh secret set ANDROID_RELEASE_KEYSTORE_BASE64`.

Without the Android secrets, the APK is signed with a throwaway key (the
artifact name ends in `-throwaway-key`), so it cannot update an installed
release. Without the iOS secrets, the archive is unsigned (`-unsigned`) and
nothing is uploaded. Archiving needs an Apple Development identity; without the
optional `.p12`, Xcode creates one through the API key on each fresh runner.
