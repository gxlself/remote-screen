#!/bin/bash
# No simctl or simulator launch; the same products directory serves both projects.
set -euo pipefail
RS_APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RS_CONFIGURATION="${1:-Debug}"
RS_SDK="${2:-iphonesimulator}"
case "$RS_CONFIGURATION" in Debug|Release) ;; *) echo 'Use Debug or Release' >&2; exit 2 ;; esac
case "$RS_SDK" in
  iphonesimulator) RS_DESTINATION='generic/platform=iOS Simulator' ;;
  iphoneos) RS_DESTINATION='generic/platform=iOS' ;;
  *) echo 'Use iphonesimulator or iphoneos' >&2; exit 2 ;;
esac
cd "$RS_APP_ROOT"
if [[ "${RS_SKIP_PODS:-0}" != 1 ]]; then
  # CocoaPods needs the initial XCFramework to inspect its platform slices.
  # Once installed, RsCore's RsMobileBuild dependency owns Rust regeneration.
  CONFIGURATION="$RS_CONFIGURATION" ./rust/build-ios.sh
  "${RS_POD:-/opt/homebrew/bin/pod}" install --project-directory=ios
fi
RS_BUILD_ROOT="${RS_DERIVED_DATA:-$RS_APP_ROOT/ios/build-$RS_CONFIGURATION-$RS_SDK}"
RS_ARGS=(-sdk "$RS_SDK" -configuration "$RS_CONFIGURATION"
  -destination "$RS_DESTINATION" -derivedDataPath "$RS_BUILD_ROOT"
  -jobs "${RS_BUILD_JOBS:-4}" "BUILD_DIR=$RS_BUILD_ROOT/Build/Products"
  CODE_SIGNING_ALLOWED=NO)
if [[ "${RS_XCODE_PROJECTS:-0}" == 1 ]]; then
  xcodebuild -project ios/Pods/Pods.xcodeproj -scheme Pods-RemoteScreen "${RS_ARGS[@]}" build
  xcodebuild -project ios/RemoteScreen.xcodeproj -scheme RemoteScreen "${RS_ARGS[@]}" build
else
  xcodebuild -workspace ios/RemoteScreen.xcworkspace -scheme RemoteScreen "${RS_ARGS[@]}" build
fi
RS_APP="$RS_BUILD_ROOT/Build/Products/$RS_CONFIGURATION-$RS_SDK/RemoteScreen.app"
echo "Remote Screen app: $RS_APP"
python3 scripts/app-size.py "$RS_APP"
