#!/usr/bin/env python3
"""Static checks for the Remote Host WiX v5 MSI source.

This checker intentionally uses only the Python standard library.  It does
not invoke WiX and therefore runs on the Linux build workers as well as on a
Windows packaging host.
"""

from __future__ import annotations

import argparse
import base64
import re
import shutil
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Iterable


WIX_NS = "http://wixtoolset.org/schemas/v4/wxs"
FW_NS = "http://wixtoolset.org/schemas/v4/wxs/firewall"
WIX = "{" + WIX_NS + "}"
FW = "{" + FW_NS + "}"

EXPECTED_UPGRADE = "3F4C2B75-8C75-49A7-9169-DBB4728F5188"
EXPECTED_BUNDLE_UPGRADE = "997D839F-5A62-4AF0-9195-660749D25152"
TAILNET_UDP = "Remote Host (tailnet) UDP"
TAILNET_TCP = "Remote Host (tailnet) TCP"
FIREWALL_CLEANUP = (
    "Get-NetFirewallRule -Group 'Remote Host' -ErrorAction SilentlyContinue | "
    "Where-Object { $_.DisplayName -notlike 'Remote Host (tailnet)*' } | "
    "Remove-NetFirewallRule -ErrorAction SilentlyContinue"
)
FIREWALL_CLEANUP_B64 = base64.b64encode(FIREWALL_CLEANUP.encode("utf-16le")).decode("ascii")


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def children(root: ET.Element, name: str) -> list[ET.Element]:
    return [element for element in root.iter() if local_name(element.tag) == name]


def formal_view(source: str) -> str:
    """Select the normal (SkipServiceStart undefined) preprocessor branch."""
    pattern = re.compile(
        r"<\?ifdef\s+SkipServiceStart\s*\?>(.*?)<\?else\s*\?>(.*?)<\?endif\s*\?>",
        re.DOTALL,
    )
    return pattern.sub(lambda match: match.group(2), source)


def parse_xml(source: str, label: str, failures: list[str]) -> ET.Element | None:
    try:
        return ET.fromstring(source)
    except ET.ParseError as exc:
        failures.append(f"XML: {label} is not well-formed ({exc})")
        return None


def parse_defines(wxi: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for match in re.finditer(r'<\?define\s+([A-Za-z_][\w]*)\s*=\s*"([^"]*)"\s*\?>', wxi):
        values[match.group(1)] = match.group(2)
    return values


def read_text(path: Path, failures: list[str], label: str) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        failures.append(f"input: cannot read {label} ({exc})")
        return ""


def check_xml_and_namespaces(wxs_root: ET.Element | None, wxi_root: ET.Element | None, failures: list[str]) -> None:
    if wxs_root is not None and wxs_root.tag != WIX + "Wix":
        failures.append("XML namespace: RemoteHost.wxs root must be the v4 Wix element")
    if wxi_root is not None and wxi_root.tag != WIX + "Include":
        failures.append("XML namespace: Product.wxi root must be the v4 Include element")


def check_variables(wxs_source: str, wxi_source: str, failures: list[str]) -> None:
    defines = set(parse_defines(wxi_source))
    allowed = defines | {"Version", "StageDir"}
    vars_found = set(re.findall(r"\$\(var\.([A-Za-z_][\w]*)\)", wxs_source))
    unknown = sorted(vars_found - allowed)
    if unknown:
        failures.append("variable: unknown $(var.X) names: " + ", ".join(unknown))
    if re.search(r"\$\(env\.[^)]+\)", wxs_source):
        failures.append("variable: $(env.X) syntax is forbidden")
    if re.search(r"\$\((?!var\.)[^)]+\)", wxs_source):
        failures.append("variable: $(X) shorthand is forbidden")


def check_product_codes(wxi_source: str, failures: list[str]) -> None:
    defines = parse_defines(wxi_source)
    guid_re = re.compile(r"^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$")
    upgrade = defines.get("UpgradeCode", "")
    bundle = defines.get("BundleUpgradeCode", "")
    if not guid_re.fullmatch(upgrade):
        failures.append("Product.wxi: UpgradeCode is not a valid GUID")
    if not guid_re.fullmatch(bundle):
        failures.append("Product.wxi: BundleUpgradeCode is not a valid GUID")
    if upgrade == bundle:
        failures.append("Product.wxi: upgrade codes must be different")
    if upgrade != EXPECTED_UPGRADE:
        failures.append("Product.wxi: UpgradeCode does not match the contract")
    if bundle != EXPECTED_BUNDLE_UPGRADE:
        failures.append("Product.wxi: BundleUpgradeCode does not match the contract")


def check_ascii(wxs_source: str, failures: list[str]) -> None:
    without_comments = re.sub(r"<!--.*?-->", "", wxs_source, flags=re.DOTALL)
    if any(ord(char) > 127 for char in without_comments):
        failures.append("ASCII: MSI strings and source outside XML comments must be ASCII")


def check_package(root: ET.Element | None, wxs_source: str, failures: list[str]) -> None:
    if root is None:
        return
    packages = children(root, "Package")
    if len(packages) != 1:
        failures.append("Package: expected exactly one Package element")
        return
    package = packages[0]
    if package.get("Scope") != "perMachine":
        failures.append("Package: Scope must be perMachine")
    try:
        if int(package.get("InstallerVersion", "0")) < 500:
            failures.append("Package: InstallerVersion must be at least 500")
    except ValueError:
        failures.append("Package: InstallerVersion must be numeric")
    if package.get("UpgradeCode") != "$(var.UpgradeCode)":
        failures.append("Package: UpgradeCode must be $(var.UpgradeCode)")
    upgrades = children(root, "MajorUpgrade")
    if len(upgrades) != 1 or upgrades[0].get("AllowDowngrades") != "yes":
        failures.append("AllowDowngrades: MajorUpgrade must allow downgrades")
    else:
        if "DowngradeErrorMessage" in upgrades[0].attrib:
            failures.append("AllowDowngrades: DowngradeErrorMessage must be absent")
        if "AllowSameVersionUpgrades" in upgrades[0].attrib:
            failures.append("AllowDowngrades: AllowSameVersionUpgrades must be absent")
    if "ScheduleReboot" in wxs_source or "ForceReboot" in wxs_source or re.search(r'Id\s*=\s*"REBOOT"', wxs_source):
        failures.append("Package: ScheduleReboot, ForceReboot, and REBOOT are forbidden")


def check_service(
    formal_root: ET.Element | None, original_source: str, product_defines: dict[str, str], repo: Path, failures: list[str]
) -> None:
    if formal_root is None:
        return
    installs = children(formal_root, "ServiceInstall")
    if len(installs) != 1:
        failures.append("Service: expected one ServiceInstall")
    else:
        install = installs[0]
        expected = {
            "Name": "$(var.ServiceName)",
            "Account": "LocalSystem",
            "Start": "auto",
            "Arguments": "$(var.ServiceArgs)",
        }
        for attr, value in expected.items():
            if install.get(attr) != value:
                failures.append(f"Service: ServiceInstall {attr} must be {value}")
        failures_count = sum(1 for element in children(install, "Failure") if element.get("Action") == "restartService")
        if failures_count != 3:
            failures.append("Service: ServiceConfigFailureActions must contain three restartService failures")
        configs = children(install, "ServiceConfig")
        if not any(element.get("FailureActionsWhen") == "failedToStopOrReturnedError" for element in configs):
            failures.append(
                "Service: ServiceConfig FailureActionsWhen must be failedToStopOrReturnedError (restart on non-crash failures)"
            )
    controls = children(formal_root, "ServiceControl")
    if len(controls) != 1:
        failures.append("ServiceControl: formal view must contain exactly one ServiceControl")
    else:
        control = controls[0]
        for attr, value in {"Start": "install", "Stop": "both", "Remove": "uninstall"}.items():
            if control.get(attr) != value:
                failures.append(f"ServiceControl: formal control must have {attr}=\"{value}\"")
    if not re.search(r"<\?ifdef\s+SkipServiceStart\?>.*?<\?else\?>.*?<\?endif\?>", original_source, flags=re.DOTALL):
        failures.append("ServiceControl: SkipServiceStart if/else branch is missing")
    else:
        branch = re.search(r"<\?ifdef\s+SkipServiceStart\?>(.*?)<\?else\?>", original_source, flags=re.DOTALL)
        if branch and re.search(r"<ServiceControl\b[^>]*\bStart\s*=", branch.group(1)):
            failures.append("ServiceControl: SkipServiceStart branch must omit Start")
    appendix = repo / "docs/dev/10-appendix.md"
    appendix_text = read_text(appendix, failures, "docs/dev/10-appendix.md")
    if "RemoteHostService" not in appendix_text or product_defines.get("ServiceName") != "RemoteHostService":
        failures.append("Service: appendix and Product.wxi must name RemoteHostService")


def check_files(root: ET.Element | None, repo: Path, product_defines: dict[str, str], failures: list[str], warnings: list[str]) -> None:
    if root is None:
        return
    files = {element.get("Id"): element for element in children(root, "File")}
    expected = {
        "RsHostExe": "$(var.StageDir)\\$(var.ServiceExe)",
        "RsHostSessionExe": "$(var.StageDir)\\$(var.SessionExe)",
        "TrayExe": "$(var.StageDir)\\$(var.TrayExe)",
    }
    for file_id, source in expected.items():
        if files.get(file_id) is None or files[file_id].get("Source") != source:
            failures.append(f"Files: {file_id} must use Source={source}")
    cargo = read_text(repo / "crates/rs-host-agent/Cargo.toml", failures, "crates/rs-host-agent/Cargo.toml")
    bin_sections = re.findall(r"\[\[bin\]\](.*?)(?=\n\[\[|\Z)", cargo, flags=re.DOTALL)
    bin_names = [match.group(1) for section in bin_sections if (match := re.search(r'^name\s*=\s*"([^"]+)"', section, flags=re.MULTILINE))]
    if bin_names and product_defines.get("ServiceExe") != bin_names[0] + ".exe":
        warnings.append("service executable differs from rs-host-agent bin name")
    if not (repo / "crates/host-win/src/bin/rs-host-session.rs").exists():
        warnings.append("rs-host-session.rs is not present yet")


def check_firewall(root: ET.Element | None, repo: Path, product_defines: dict[str, str], failures: list[str], warnings: list[str]) -> None:
    if root is None:
        return
    rules = [element for element in root.iter() if element.tag == FW + "FirewallException"]
    if len(rules) != 2:
        failures.append("Firewall: expected exactly two FirewallException elements")
        return
    listen = product_defines.get("ListenPort")
    listen_source = read_text(repo / "crates/host-core/src/listen.rs", failures, "crates/host-core/src/listen.rs")
    default_match = re.search(r"port:\s*(\d+)", listen_source)
    if not default_match or listen != default_match.group(1):
        failures.append("ListenPort: Product.wxi ListenPort must match listen.rs default")
    names = {rule.get("Name") for rule in rules}
    if names != {TAILNET_UDP, TAILNET_TCP}:
        failures.append("Firewall: rule names must be the tailnet UDP and TCP names")
    for rule in rules:
        expected_protocol = "udp" if rule.get("Name") == TAILNET_UDP else "tcp"
        if (
            rule.get("Port") != "$(var.ListenPort)"
            or rule.get("Program") != "[#RsHostExe]"
            or rule.get("Protocol") != expected_protocol
            or rule.get("Profile") != "all"
            or rule.get("Grouping") != "Remote Host"
            or rule.get("IgnoreFailure") != "no"
        ):
            failures.append("Firewall: rules must use the ListenPort variable, RsHostExe, protocol, profile all, and group Remote Host")
        if not any(address.get("Value") == "100.64.0.0/10" for address in rule if address.tag == FW + "RemoteAddress"):
            failures.append("Firewall: each rule must include remote tailnet range 100.64.0.0/10")
        local_values = {address.get("Value") for address in rule if address.tag == FW + "LocalAddress"}
        if local_values != {"100.64.0.0/10", "fd7a:115c:a1e0::/48"}:
            failures.append("Firewall: each rule must include both v5 tailnet LocalAddress values")
    model_path = repo / "crates/host-win/src/firewall_model.rs"
    model = read_text(model_path, failures, "crates/host-win/src/firewall_model.rs")
    if not model.strip():
        warnings.append("firewall_model.rs is empty; using contract tailnet rule names")
    else:
        for constant, expected in (("TAILNET_RULE_UDP", TAILNET_UDP), ("TAILNET_RULE_TCP", TAILNET_TCP)):
            match = re.search(rf"{constant}\s*:\s*&?str\s*=\s*\"([^\"]+)\"", model)
            if not match:
                failures.append(f"Firewall: could not parse {constant} from firewall_model.rs")
            elif match.group(1) != expected:
                failures.append(f"Firewall: {constant} does not match the MSI rule name")


def check_sas(root: ET.Element | None, repo: Path, failures: list[str]) -> None:
    if root is None:
        return
    properties = {element.get("Id"): element for element in children(root, "Property")}
    components = {element.get("Id"): element for element in children(root, "Component")}
    for component_id, value, condition in (
        ("SasServices", "1", 'NOT SASGEN OR SASGEN="#0"'),
        ("SasServicesAndEase", "3", 'SASGEN="#2"'),
    ):
        component = components.get(component_id)
        if component is None:
            failures.append(f"SAS: missing {component_id} component")
            continue
        if (
            component.get("Condition") != condition
            or component.get("Permanent") != "yes"
            or component.get("Bitness") != "always64"
        ):
            failures.append(f"SAS: {component_id} has the wrong condition or permanence")
        values = [element for element in children(component, "RegistryValue") if element.get("Name") == "SoftwareSASGeneration"]
        if len(values) != 1 or values[0].get("Value") != value:
            failures.append(f"SAS: {component_id} must write SoftwareSASGeneration={value}")
    sasgen = properties.get("SASGEN")
    if sasgen is None or not any(
        element.get("Name") == "SoftwareSASGeneration"
        for element in sasgen
        if local_name(element.tag) == "RegistrySearch"
    ):
        failures.append("SAS: SASGEN must search SoftwareSASGeneration")
    input_raw = read_text(repo / "crates/host-win/src/input_raw.rs", failures, "crates/host-win/src/input_raw.rs")
    if not re.search(r"matches!\s*\(value,\s*Some\(1\s*\|\s*3\)\)", input_raw):
        failures.append("SAS: sas_policy_allows must accept values 1 and 3")


def permission_names(repo: Path, failures: list[str]) -> set[str]:
    source = read_text(repo / "crates/host-core/src/pairfile_store.rs", failures, "crates/host-core/src/pairfile_store.rs")
    return set(re.findall(r'"(view_only|control|unattended)"', source))


def cleanup_file_names(repo: Path, failures: list[str]) -> tuple[str, str, str]:
    store = read_text(repo / "crates/host-core/src/store.rs", failures, "crates/host-core/src/store.rs")
    pairfiles = read_text(repo / "crates/host-core/src/pairfile_store.rs", failures, "crates/host-core/src/pairfile_store.rs")
    password = read_text(repo / "crates/host-core/src/pwpair.rs", failures, "crates/host-core/src/pwpair.rs")
    paired_match = re.search(r'PAIRED_FILE\s*:\s*&str\s*=\s*"([^"]+)"', store)
    pairfiles_match = re.search(r'PAIRFILES_FILE\s*:\s*&str\s*=\s*"([^"]+)"', pairfiles)
    password_match = re.search(r'LOCK_FILE\s*:\s*&str\s*=\s*"([^"]+)"', password)
    return (
        paired_match.group(1) if paired_match else "paired.json",
        pairfiles_match.group(1) if pairfiles_match else "pairfiles.json",
        password_match.group(1) if password_match else "password_lock.json",
    )


def check_pairfile(root: ET.Element | None, repo: Path, failures: list[str]) -> None:
    if root is None:
        return
    properties = {element.get("Id"): element for element in children(root, "Property")}
    for prop in ("PAIRFILE", "PAIRFILE_PERMISSION", "PAIRFILE_LABEL", "REMOVEDATA"):
        if prop not in properties or properties[prop].get("Secure") != "yes":
            failures.append(f"PAIRFILE: {prop} must be declared Secure=\"yes\"")
    launches = children(root, "Launch")
    absolute_launch = next((element for element in launches if "PAIRFILE <<" in element.get("Condition", "")), None)
    if absolute_launch is None or 'PAIRFILE << "\\\\"' not in absolute_launch.get("Condition", "") or 'PAIRFILE >< ":\\"' not in absolute_launch.get("Condition", ""):
        failures.append("PAIRFILE: absolute path Launch condition is missing")
    permission_launch = next((element for element in launches if "PAIRFILE_PERMISSION" in element.get("Condition", "")), None)
    if permission_launch is None:
        failures.append("PAIRFILE: permission Launch condition is missing")
    else:
        values = set(re.findall(r'PAIRFILE_PERMISSION=\"([^\"]+)\"', permission_launch.get("Condition", "")))
        expected = permission_names(repo, failures)
        if values != expected:
            failures.append("PAIRFILE: Launch permission values do not match permission_name")
    registries = [element for element in children(root, "RegistryKey") if element.get("Key") == "$(var.DeployRegKey)"]
    if len(registries) != 1:
        failures.append("PAIRFILE: Deploy registry key must be $(var.DeployRegKey)")
    elif {element.get("Name") for element in children(registries[0], "RegistryValue")} != {
        "PairFile",
        "PairFilePermission",
        "PairFileLabel",
    }:
        failures.append("PAIRFILE: Deploy registry key must contain PairFile, PairFilePermission, and PairFileLabel")


def check_cleanup(
    root: ET.Element | None,
    wxs_source: str,
    repo: Path,
    product_defines: dict[str, str],
    failures: list[str],
) -> None:
    if root is None:
        return
    actions = {element.get("Id"): element for element in children(root, "CustomAction")}
    for action_id in ("RsCleanupPairing", "RsCleanupFirewall", "RsPurgeData"):
        action = actions.get(action_id)
        if action is None:
            failures.append(f"cleanup: missing {action_id}")
            continue
        if action.get("Execute") != "deferred" or action.get("Impersonate") != "no":
            failures.append(f"cleanup: {action_id} must be deferred and non-impersonating")
        condition = action.get("Condition", "")
        if 'REMOVE~="ALL"' not in condition or "NOT UPGRADINGPRODUCTCODE" not in condition:
            failures.append(f"cleanup: {action_id} must exclude upgrades")
        if action_id == "RsPurgeData" and 'REMOVEDATA="1"' not in condition:
            failures.append("cleanup: RsPurgeData must require REMOVEDATA=1")
    commands = {element.get("Id"): element.get("Value", "") for element in children(root, "SetProperty")}
    pairing = commands.get("RsCleanupPairingCmdLine", "")
    for file_name in cleanup_file_names(repo, failures):
        if file_name not in pairing:
            failures.append(f"cleanup: pairing command must remove {file_name}")
    paths = read_text(repo / "crates/host-win/src/paths.rs", failures, "crates/host-win/src/paths.rs")
    data_dir_match = re.search(r'let\s+data_dir\s*=\s*format!\(r"\{root\}\\([^" ]+)"\)', paths)
    data_dir_name = data_dir_match.group(1) if data_dir_match else ""
    if data_dir_name != "RemoteHost" or product_defines.get("DataDirName") != data_dir_name or "$(var.DataDirName)" not in pairing:
        failures.append("cleanup: pairing command must target the RemoteHost data directory")
    firewall = commands.get("RsCleanupFirewallCmdLine", "")
    encoded = re.search(r"-EncodedCommand\s+([^\s\"]+)", firewall)
    if not encoded:
        failures.append("EncodedCommand: firewall cleanup must provide a base64 command")
    else:
        try:
            decoded = base64.b64decode(encoded.group(1)).decode("utf-16le")
        except (ValueError, UnicodeError) as exc:
            failures.append(f"EncodedCommand: invalid UTF-16LE base64 ({exc})")
        else:
            if decoded != FIREWALL_CLEANUP:
                failures.append("EncodedCommand: firewall cleanup script does not match the contract")
    for element in root.iter():
        if local_name(element.tag) in {"RemoveFile", "RemoveFolder", "RemoveFolderEx"}:
            text = ET.tostring(element, encoding="unicode")
            if "DataDirName" in text or "CommonAppDataFolder" in text or "RemoteHost" in text:
                failures.append("RemoveFolder: data directory must not use RemoveFile/RemoveFolder/RemoveFolderEx")
    if re.search(r"<\s*(?:RemoveFile|RemoveFolder|RemoveFolderEx)\b[^>]*(?:DataDirName|CommonAppDataFolder|RemoteHost)", wxs_source):
        failures.append("RemoveFolder: data directory cleanup table entry is forbidden")


REMOVED_NOTIFY_IDS = {"RsNotifyHub", "RsNotifyHubCmdLine"}


def check_no_notify(root: ET.Element | None, wxs_source: str, failures: list[str]) -> None:
    if root is None:
        return
    ids = {element.get("Id") for element in children(root, "CustomAction")}
    ids |= {element.get("Id") for element in children(root, "SetProperty")}
    if ids & REMOVED_NOTIFY_IDS or "--notify-uninstall" in wxs_source:
        failures.append("notify: the MSI must not run the removed --notify-uninstall action")


def check_msi(repo: Path, root: ET.Element | None, failures: list[str], warnings: list[str]) -> None:
    source = read_text(repo / "crates/host-win/src/msi.rs", failures, "crates/host-win/src/msi.rs")
    if not source.strip():
        warnings.append("msi.rs not merged yet")
        return
    if "/qn" not in source or "/norestart" not in source:
        failures.append("W11A: msi.rs install arguments must contain /qn and /norestart")
    if root is not None:
        upgrades = children(root, "MajorUpgrade")
        if len(upgrades) != 1 or upgrades[0].get("AllowDowngrades") != "yes":
            failures.append("W11A: MSI must retain AllowDowngrades=yes")


def check_all(repo: Path, wxs_path: Path | None = None) -> tuple[list[str], list[str]]:
    failures: list[str] = []
    warnings: list[str] = []
    win_dir = repo / "tools/release/win"
    wxs_path = wxs_path or win_dir / "RemoteHost.wxs"
    wxi_path = win_dir / "Product.wxi"
    wxs_source = read_text(wxs_path, failures, str(wxs_path))
    wxi_source = read_text(wxi_path, failures, str(wxi_path))
    formal_source = formal_view(wxs_source)
    # Parse the original too: a malformed inactive preprocessor branch is
    # still a malformed source file and should be reported.
    parse_xml(wxs_source, "RemoteHost.wxs (source)", failures)
    wxs_root = parse_xml(formal_source, "RemoteHost.wxs", failures)
    wxi_root = parse_xml(wxi_source, "Product.wxi", failures)
    check_xml_and_namespaces(wxs_root, wxi_root, failures)
    check_variables(wxs_source, wxi_source, failures)
    check_product_codes(wxi_source, failures)
    check_ascii(wxs_source, failures)
    check_package(wxs_root, wxs_source, failures)
    defines = parse_defines(wxi_source)
    check_service(wxs_root, wxs_source, defines, repo, failures)
    check_files(wxs_root, repo, defines, failures, warnings)
    check_firewall(wxs_root, repo, defines, failures, warnings)
    check_sas(wxs_root, repo, failures)
    check_pairfile(wxs_root, repo, failures)
    check_cleanup(wxs_root, wxs_source, repo, defines, failures)
    check_no_notify(wxs_root, wxs_source, failures)
    check_msi(repo, wxs_root, failures, warnings)
    return failures, warnings


def copy_test_repo(repo: Path, directory: Path) -> Path:
    """Copy only checker inputs into a small temporary repository."""
    for relative in (
        "tools/release/win/RemoteHost.wxs",
        "tools/release/win/Product.wxi",
        "docs/dev/10-appendix.md",
        "crates/rs-host-agent/Cargo.toml",
        "crates/host-core/src/listen.rs",
        "crates/host-core/src/store.rs",
        "crates/host-core/src/pairfile_store.rs",
        "crates/host-core/src/pwpair.rs",
        "crates/host-win/src/paths.rs",
        "crates/host-win/src/input_raw.rs",
        "crates/host-win/src/msi.rs",
        "crates/host-win/src/firewall_model.rs",
    ):
        source = repo / relative
        target = directory / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.exists():
            shutil.copy2(source, target)
    return directory


def self_test(repo: Path) -> list[str]:
    failures: list[str] = []
    base = repo / "tools/release/win/RemoteHost.wxs"
    original = base.read_text(encoding="utf-8")

    def expect(label: str, mutate, keyword: str, mutate_wxi=None) -> None:
        with tempfile.TemporaryDirectory(prefix="check-wix-") as temp_name:
            temp_repo = copy_test_repo(repo, Path(temp_name))
            wxs = temp_repo / "tools/release/win/RemoteHost.wxs"
            wxs.write_text(mutate(original), encoding="utf-8")
            if mutate_wxi is not None:
                wxi = temp_repo / "tools/release/win/Product.wxi"
                wxi.write_text(mutate_wxi(wxi.read_text(encoding="utf-8")), encoding="utf-8")
            found, _ = check_all(temp_repo, wxs)
            if not any(keyword.lower() in item.lower() for item in found):
                failures.append(f"SELF-TEST {label}: expected {keyword!r} failure")

    expect("AllowDowngrades", lambda text: text.replace('AllowDowngrades="yes"', ""), "AllowDowngrades")
    expect(
        "ListenPort",
        lambda text: text,
        "ListenPort",
        lambda text: text.replace('ListenPort = "7400"', 'ListenPort = "7401"'),
    )
    expect(
        "cleanup upgrade guard",
        lambda text: text.replace(' AND NOT UPGRADINGPRODUCTCODE', ""),
        "cleanup",
    )
    expect("ASCII", lambda text: text.replace("Remote Host remote access service", "Remote Host caf\u00e9 service"), "ASCII")
    expect("XML", lambda text: text.replace("</ServiceInstall>", "", 1), "XML")
    expect("variable", lambda text: text.replace('$(var.ProductName)', '$(var.Foo)', 1), "variable")
    expect(
        "EncodedCommand",
        lambda text: re.sub(r"(-EncodedCommand\s+)[^\s\"]+", r"\g<1>QQ==", text, count=1),
        "EncodedCommand",
    )
    expect("RemoveFolder", lambda text: text.replace("</ComponentGroup>", '<RemoveFolder Id="RemoveData" Directory="$(var.DataDirName)" /></ComponentGroup>', 1), "RemoveFolder")
    expect("notify", lambda text: text.replace("</Fragment>", '<CustomAction Id="RsNotifyHub" BinaryRef="Wix4UtilCA_X64" DllEntry="WixQuietExec64" After="StopServices" /></Fragment>', 1), "notify")

    def remove_start(text: str) -> str:
        match = re.search(r"(<\?else\?>\s*<ServiceControl\b[^>]*?)\s+Start=\"install\"", text, flags=re.DOTALL)
        return text if not match else text[: match.start()] + match.group(1) + text[match.end() :]

    expect("ServiceControl", remove_start, "ServiceControl")
    return failures


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=None, help="repository root (default: inferred from this script)")
    parser.add_argument("--wxs", type=Path, default=None, help="RemoteHost.wxs path")
    parser.add_argument("--self-test", action="store_true", help="run mutation tests after the normal check")
    args = parser.parse_args(list(argv) if argv is not None else None)
    script_repo = Path(__file__).resolve().parents[3]
    repo = (args.repo or script_repo).resolve()
    wxs = args.wxs.resolve() if args.wxs else None
    failures, warnings = check_all(repo, wxs)
    for warning in warnings:
        print(f"WARN: {warning}")
    for failure in failures:
        print(f"FAIL: {failure}")
    if failures:
        return 1
    if args.self_test:
        self_failures = self_test(repo)
        for failure in self_failures:
            print(f"FAIL: {failure}")
        if self_failures:
            return 1
        print("SELF-TEST OK")
    else:
        print("OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
