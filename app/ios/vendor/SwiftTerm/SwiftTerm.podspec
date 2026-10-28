# Local podspec for the vendored SwiftTerm release (see VENDOR.md).
Pod::Spec.new do |s|
  s.name = 'SwiftTerm'
  s.version = '1.20.0'
  s.summary = 'VT100/Xterm terminal emulator (vendored).'
  s.homepage = 'https://github.com/migueldeicaza/SwiftTerm'
  s.license = { :type => 'MIT', :file => 'LICENSE' }
  s.author = 'Miguel de Icaza'
  s.source = { :path => '.' }
  s.ios.deployment_target = '14.0'
  # Upstream builds in Swift 5 language mode (swiftLanguageModes: [.v5]).
  s.swift_version = '5.0'
  # Same file set SwiftPM compiles for Apple platforms; Mac/ sources are
  # guarded by #if os(macOS).
  s.source_files = 'Sources/SwiftTerm/**/*.swift', 'Generated/*.swift'
  # Metal rendering is opt-in; the renderer compiles this source at runtime
  # when no metallib is bundled.
  s.resources = 'Sources/SwiftTerm/Apple/Metal/Shaders.metal'
  s.frameworks = 'UIKit', 'CoreText', 'CoreGraphics', 'QuartzCore', 'Metal', 'MetalKit'
end
