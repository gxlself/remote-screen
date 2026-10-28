# SwiftTerm (vendored)

- Upstream: https://github.com/migueldeicaza/SwiftTerm
- Release: `v1.20.0` (2026-08-18), commit `5d14406844143538cd8f8851d2d8a67c1fe443e5`
- License: MIT (`LICENSE`, unchanged)

## What is here

- `Sources/SwiftTerm/`: upstream `Sources/SwiftTerm/` unchanged, minus
  `Documentation.docc/` and `Mac/README.md` (both excluded by upstream builds too).
- `Generated/SwiftTermBuildInfo.swift`: replaces the output of upstream's SwiftPM
  build-tool plugin, which CocoaPods cannot run. Values are pinned to this release.
- `SwiftTerm.podspec`: local podspec; the app's Podfile uses
  `pod 'SwiftTerm', :path => 'vendor/SwiftTerm'` so builds work offline.

## Updating

Replace `Sources/SwiftTerm/` with the new release's tree (same exclusions), update the
tag and commit in `Generated/SwiftTermBuildInfo.swift`, the podspec version and this
file, then run `pod install` and rebuild.
