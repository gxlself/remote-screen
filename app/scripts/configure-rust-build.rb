# Workspace crates rs-mobile links, found by following `<crate>.workspace = true`
# dependencies whose crates/<crate> directory exists. A hard-coded list went
# stale as rs-recording, rs-ssh, rs-vnc, rs-rdp and others were added.
def rust_crate_closure(repo_root, root)
  seen = []
  queue = [root]
  until queue.empty?
    name = queue.shift
    next if seen.include?(name)
    manifest = File.join(repo_root, 'crates', name, 'Cargo.toml')
    next unless File.file?(manifest)
    seen << name
    section = nil
    File.foreach(manifest, encoding: 'UTF-8') do |line|
      if (header = line[/^\[([^\]]+)\]/, 1])
        section = header
        next
      end
      next unless section && section.match?(/(^|\.)(dependencies|build-dependencies)$/)
      dep = line[/^([A-Za-z0-9_-]+)\s*(?:\.workspace\s*=\s*true|=\s*\{[^}]*workspace\s*=\s*true)/, 1]
      queue << dep if dep && File.directory?(File.join(repo_root, 'crates', dep))
    end
  end
  seen.sort
end

# Install an explicit dependency before RsCore's XCFramework copy and compile.
# An app-only phase runs too late: CocoaPods may have already copied the old lib.
def configure_rust_build(installer)
  app_root = File.expand_path('..', __dir__)
  repo_root = File.expand_path('..', app_root)
  inputs = %w[Cargo.toml Cargo.lock app/pnpm-lock.yaml app/native/core/ubrn.config.yaml]
  inputs.concat(Dir.glob(File.join(repo_root, 'app/rust/*')).select { |path|
    File.file?(path) && !path.end_with?('.xcfilelist')
  }.map { |path| path.delete_prefix(repo_root + '/') })
  rust_crate_closure(repo_root, 'rs-mobile').each do |name|
    base = "crates/#{name}"
    inputs.concat(["#{base}/Cargo.toml", "#{base}/src"])
    inputs.concat(Dir.glob(File.join(repo_root, base, '{build.rs,src/**/*,include/**/*}'))
      .select { |path| File.file?(path) }.map { |path| path.delete_prefix(repo_root + '/') })
  end
  inputs.concat(Dir.glob(File.join(repo_root, 'proto/rsp/**/*.proto'))
    .map { |path| path.delete_prefix(repo_root + '/') })
  list = inputs.uniq.sort.map { |path| "$(PODS_ROOT)/../../../#{path}\n" }.join
  list_path = File.join(app_root, 'rust/inputs.xcfilelist')
  File.write(list_path, list) unless File.exist?(list_path) && File.read(list_path) == list

  project = installer.pods_project
  rust = project.targets.find { |t| t.name == 'RsMobileBuild' } ||
    project.new_aggregate_target('RsMobileBuild', [], :ios, '17.0')
  phase = rust.shell_script_build_phases.find { |p| p.name == 'Build rs-mobile and UniFFI bindings' } ||
    rust.new_shell_script_build_phase('Build rs-mobile and UniFFI bindings')
  phase.shell_script = "set -e\n\"$PODS_ROOT/../../rust/build-ios.sh\"\n"
  phase.input_file_list_paths = ['${PODS_ROOT}/../../rust/inputs.xcfilelist']
  phase.output_file_list_paths = ['${PODS_ROOT}/../../rust/outputs.xcfilelist']
  phase.show_env_vars_in_log = '0'
  rust.build_configurations.each do |cfg|
    cfg.build_settings['ENABLE_USER_SCRIPT_SANDBOXING'] = 'NO'
    # Keep the Rust profile aligned with the Xcode configuration. A caller can
    # still override it explicitly for local profiling or CI experiments.
    cfg.build_settings['RS_MOBILE_PROFILE'] = '$(CONFIGURATION)'
    cfg.build_settings['PODS_ROOT'] = '${SRCROOT}'
  end
  core = project.targets.find { |t| t.name == 'RsCore' }
  core.add_dependency(rust) unless core.dependencies.any? { |d| d.target == rust }
end
