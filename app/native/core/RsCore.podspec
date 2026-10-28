require 'json'
package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name = 'RsCore'
  s.version = package['version']
  s.summary = package['description']
  s.homepage = package['homepage']
  s.license = package['license']
  s.author = package['author']
  s.source = { :git => package['repository']['url'], :tag => s.version.to_s }
  s.platform = :ios, '17.0'

  # CocoaPods restricts globs to the pod root. These in-root symlinks expose
  # the app-owned generated glue and framework without duplicating either.
  s.source_files = 'ios/*.{h,mm}', 'cpp/*.{h,hpp,cpp}', 'cpp/generated/*.{h,hpp,cpp}'
  s.vendored_frameworks = 'ios/RsMobile.xcframework'
  # Go runtime of the embedded tailnet (inside librs_mobile.a when built with tailnet-static).
  s.libraries = 'resolv'
  s.frameworks = 'Security', 'CoreFoundation'
  s.dependency 'uniffi-bindgen-react-native', '0.31.0-6'
  install_modules_dependencies(s)
end
