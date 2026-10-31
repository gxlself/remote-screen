# Idempotent additions to the RN CLI-generated iOS project. Run with the Ruby
# and GEM_HOME used by the installed CocoaPods (no system gem installation).
require 'xcodeproj'

app_root = File.expand_path('..', __dir__)
project = Xcodeproj::Project.open(File.join(app_root, 'ios/RemoteScreen.xcodeproj'))
target = project.targets.find { |item| item.name == 'RemoteScreen' }
target.resources_build_phase.files.each do |item|
  item.remove_from_project if ['LaunchScreen.storyboard', 'Images.xcassets'].any? { |name|
    item.file_ref&.path&.end_with?(name)
  }
end
project.files.select { |ref|
  ['LaunchScreen.storyboard', 'Images.xcassets'].any? { |name| ref.path&.end_with?(name) }
}.each(&:remove_from_project)

# Keep the two source trees below in sync with the files on disk.  CocoaPods
# invokes this script after every install, so all of the helpers deliberately
# reuse existing groups and file references instead of creating duplicates.
def ensure_group(parent, name, path)
  group = parent.children.find { |child|
    child.isa == 'PBXGroup' && (child.name == name || child.path == name)
  }
  group ||= parent.new_group(name)
  group.name = name
  group.path = path
  group.source_tree = '<group>'
  group
end

def source_file_type(extension)
  {
    '.swift' => 'sourcecode.swift',
    '.c' => 'sourcecode.c.c',
    '.m' => 'sourcecode.c.objc',
    '.mm' => 'sourcecode.cpp.objcpp',
    '.h' => 'sourcecode.c.h',
  }.fetch(extension)
end

def descendants(group)
  group.children.flat_map do |child|
    child.isa == 'PBXGroup' ? descendants(child) : [child]
  end
end

def remove_group_and_build_files(group, target)
  descendants(group).each do |ref|
    target.source_build_phase.files.select { |build_file|
      build_file.file_ref == ref
    }.each(&:remove_from_project)
  end
  group.remove_from_project
end

def clean_missing_files(group, target)
  group.children.dup.each do |child|
    if child.isa == 'PBXGroup'
      clean_missing_files(child, target)
      child.remove_from_project if child.children.empty?
    elsif !File.file?(child.real_path.to_s)
      target.source_build_phase.files.select { |build_file|
        build_file.file_ref == child
      }.each(&:remove_from_project)
      child.remove_from_project
    end
  end
end

def register_source_tree(root, root_path, files, target)
  clean_missing_files(root, target)
  files.sort.each do |file|
    relative = file.delete_prefix("#{root_path}/")
    parts = relative.split(File::SEPARATOR)
    basename = parts.pop
    group = parts.reduce(root) do |parent, directory|
      ensure_group(parent, directory, directory)
    end
    ref = group.files.find { |item| item.path == basename } || group.new_file(basename)
    ref.name = basename
    ref.path = basename
    ref.last_known_file_type = source_file_type(File.extname(file))
    unless File.extname(file) == '.h' || target.source_build_phase.files_references.include?(ref)
      target.add_file_references([ref])
    end
  end
end

native_root_path = File.join(app_root, 'ios/RemoteScreen/Native')
remote_screen_group = project.main_group.find_subpath('RemoteScreen', true)
native_root = ensure_group(remote_screen_group, 'Native', 'RemoteScreen/Native')
# Older versions of this script briefly created a second Native group at the
# project root. Remove that generated duplicate before registering the
# canonical group below.
project.main_group.children.dup.each do |child|
  if child != native_root && child.isa == 'PBXGroup' && child.name == 'Native' &&
      child.path == 'RemoteScreen/Native'
    remove_group_and_build_files(child, target)
  end
end
native_files = Dir.glob(File.join(native_root_path, '**/*')).select { |file|
  File.file?(file) && %w[.swift .c .m .mm .h].include?(File.extname(file))
}.map { |file| file.delete_prefix("#{native_root_path}/") }
register_source_tree(native_root, native_root_path,
                     native_files.map { |file| File.join(native_root_path, file) }, target)

# SwiftPM sources are compiled into the app target as well as the standalone
# package. Tests remain package-only and are intentionally not registered.
logic_root_path = File.join(app_root, 'ios/NativeLogic/Sources/NativeLogic')
logic_root = ensure_group(project.main_group, 'NativeLogic', 'NativeLogic/Sources/NativeLogic')
# Rehome any source references created directly under NativeLogic by an older
# script revision so that the package's feature subgroups remain accurate.
logic_files_on_disk = Dir.glob(File.join(logic_root_path, '**/*.swift')).select { |file|
  File.file?(file)
}
logic_root.children.dup.each do |child|
  if child.isa == 'PBXFileReference' &&
      logic_files_on_disk.any? { |file| File.basename(file) == child.path }
    target.source_build_phase.files.select { |build_file|
      build_file.file_ref == child
    }.each(&:remove_from_project)
    child.remove_from_project
  end
end
logic_files = Dir.glob(File.join(logic_root_path, '**/*.swift')).select { |file|
  File.file?(file)
}.map { |file| file.delete_prefix("#{logic_root_path}/") }
register_source_tree(logic_root, logic_root_path,
                     logic_files.map { |file| File.join(logic_root_path, file) }, target)
fonts_group = project.main_group.find_subpath('RemoteScreen/Fonts', true)
fonts_group.set_source_tree('<group>')
fonts_group.set_path('RemoteScreen/Fonts')
Dir.glob(File.join(app_root, 'ios/RemoteScreen/Fonts/*.ttf')).sort.each do |file|
  basename = File.basename(file)
  ref = fonts_group.files.find { |item| item.path == basename } || fonts_group.new_file(basename)
  ref.last_known_file_type = 'file'
  target.resources_build_phase.add_file_reference(ref) unless
    target.resources_build_phase.files_references.include?(ref)
end
target.build_configurations.each do |cfg|
  # The App Store needs the icon from an asset catalog (CFBundleIconName).
  cfg.build_settings['ASSETCATALOG_COMPILER_APPICON_NAME'] = 'AppIcon'
  cfg.build_settings['PRODUCT_BUNDLE_IDENTIFIER'] = if cfg.name == 'Debug'
    'ai.gxlself.remotescreen.dev'
  else
    'ai.gxlself.remotescreen'
  end
  cfg.build_settings['APP_DISPLAY_NAME'] = if cfg.name == 'Debug'
    'Remote Screen Dev'
  else
    'Remote Screen'
  end
  cfg.build_settings['PRODUCT_MODULE_NAME'] = 'RemoteScreen'
  cfg.build_settings['IPHONEOS_DEPLOYMENT_TARGET'] = '16.0'
  cfg.build_settings['SWIFT_OBJC_BRIDGING_HEADER'] = 'RemoteScreen/Native/RemoteScreen-Bridging-Header.h'
  # pnpm hoists hermes-compiler; the RN podspec's .pnpm store path is not
  # present with node-linker=hoisted.
  cfg.build_settings['HERMES_CLI_PATH'] = '$(PODS_ROOT)/../../node_modules/hermes-compiler/hermesc/osx-bin/hermesc'
  cfg.build_settings['ENABLE_USER_SCRIPT_SANDBOXING'] = 'NO'
  cfg.build_settings['EXCLUDED_ARCHS[sdk=iphonesimulator*]'] = 'x86_64'
end
assets_ref = remote_screen_group.files.find { |ref| ref.path == 'RemoteScreen/Assets.xcassets' } ||
  remote_screen_group.new_file('RemoteScreen/Assets.xcassets')
assets_ref.name = 'Assets.xcassets'
assets_ref.source_tree = '<group>'
assets_ref.last_known_file_type = 'folder.assetcatalog'
target.resources_build_phase.add_file_reference(assets_ref) unless
  target.resources_build_phase.files_references.include?(assets_ref)
art_group = project.main_group.find_subpath('RemoteScreen/Placeholders', true)
art_group.set_source_tree('<group>')
art_group.set_path('RemoteScreen/Placeholders')
# actool writes AppIcon60x60@2x.png and friends from the catalog; loose
# copies of those names would collide with its output.
legacy_icon = /\AAppIcon\d/
art_group.files.select { |ref| ref.path&.match?(legacy_icon) }.each do |ref|
  target.resources_build_phase.files.select { |build_file|
    build_file.file_ref == ref
  }.each(&:remove_from_project)
  ref.remove_from_project
end
Dir.glob(File.join(app_root, 'ios/RemoteScreen/Placeholders/*.png')).sort.each do |file|
  basename = File.basename(file)
  next if basename.match?(legacy_icon)
  ref = art_group.files.find { |item| item.path == basename } || art_group.new_file(basename)
  target.resources_build_phase.add_file_reference(ref) unless
    target.resources_build_phase.files_references.include?(ref)
end
project.save
