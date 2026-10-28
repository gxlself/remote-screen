#!/usr/bin/env python3
"""Linux-side static checks for the WiX Burn Windows release bundle."""

from __future__ import annotations

import argparse
import io
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tokenize
from pathlib import Path
from typing import Iterable
import xml.etree.ElementTree as ET


WIX_NS = "http://wixtoolset.org/schemas/v4/wxs"
BAL_NS = "http://wixtoolset.org/schemas/v4/wxs/bal"
UTIL_NS = "http://wixtoolset.org/schemas/v4/wxs/util"
PASS_THROUGH = {"PAIRFILE", "PAIRFILE_PERMISSION", "PAIRFILE_LABEL", "REMOVEDATA"}
ALLOWED_BUILD_VARS = {"Version", "MsiPath", "WebView2Bootstrapper"}
REQUIRED_PARAMETERS = {
    "Version",
    "Out",
    "SkipBuild",
    "StageDir",
    "ServicePackage",
    "ServiceBin",
    "HostUiExe",
    "WebView2Bootstrapper",
    "DownloadWebView2",
    "SkipServiceStart",
    "Sign",
    "RequireSign",
}


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def attr(element: ET.Element, name: str, namespace: str | None = None) -> str | None:
    if namespace:
        return element.get(f"{{{namespace}}}{name}")
    return element.get(name)


def strip_comments(text: str) -> str:
    return re.sub(r"<!--.*?-->", "", text, flags=re.DOTALL)


def powershell_views(source: str) -> tuple[str, str]:
    """Mask comments, then quoted strings, preserving character offsets."""
    without_comments = list(source)
    code = list(source)

    def mask(start: int, end: int, comment: bool = False) -> None:
        for index in range(start, end):
            if source[index] not in "\r\n":
                code[index] = " "
                if comment:
                    without_comments[index] = " "

    index = 0
    while index < len(source):
        if source.startswith("<#", index):
            end = source.find("#>", index + 2)
            end = len(source) if end < 0 else end + 2
            mask(index, end, True)
            index = end
        elif source[index] == "#":
            end = source.find("\n", index)
            end = len(source) if end < 0 else end
            mask(index, end, True)
            index = end
        elif source[index] in "'\"":
            quote, start = source[index], index
            index += 1
            while index < len(source):
                if quote == '"' and source[index] == chr(96):
                    index += 2
                elif source[index] == quote:
                    if quote == "'" and source[index:index + 2] == "''":
                        index += 2
                    else:
                        index += 1
                        break
                else:
                    index += 1
            mask(start, min(index, len(source)))
        else:
            index += 1
    return "".join(without_comments), "".join(code)


def closing_brace(code: str, start: int) -> int | None:
    depth = 0
    for index in range(start, len(code)):
        if code[index] == "{":
            depth += 1
        elif code[index] == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    return None


class Checks:
    def __init__(self, repo: Path, *, emit: bool = True, optional_pwsh: bool = True):
        self.repo = repo
        self.win_dir = repo / "tools" / "release" / "win"
        self.bundle_path = self.win_dir / "Bundle.wxs"
        self.product_path = self.win_dir / "Product.wxi"
        self.script_path = repo / "tools" / "release" / "win.ps1"
        self.sign_path = self.win_dir / "sign.ps1"
        self.checker_path = self.win_dir / "check_bundle.py"
        self.remote_path = self.win_dir / "RemoteHost.wxs"
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self.emit = emit
        self.optional_pwsh = optional_pwsh

    def fail(self, message: str) -> None:
        self.errors.append(message)

    def warn(self, message: str) -> None:
        self.warnings.append(message)

    def parse_xml(self, path: Path) -> ET.Element | None:
        try:
            return ET.parse(path).getroot()
        except (ET.ParseError, OSError) as exc:
            self.fail(f"{path.name} is not valid XML: {exc}")
            return None

    def check_files_and_xml(self) -> tuple[ET.Element | None, ET.Element | None]:
        if not self.bundle_path.is_file():
            self.fail("Bundle.wxs is missing")
        if not self.product_path.is_file():
            self.fail("Product.wxi is missing")
        if not self.script_path.is_file():
            self.fail("tools/release/win.ps1 is missing")
        if not self.sign_path.is_file():
            self.fail("tools/release/win/sign.ps1 is missing")
        if not self.checker_path.is_file():
            self.fail("tools/release/win/check_bundle.py is missing")
        if not self.bundle_path.is_file() or not self.product_path.is_file():
            return None, None

        bundle_root = self.parse_xml(self.bundle_path)
        product_root = self.parse_xml(self.product_path)
        if bundle_root is not None:
            if local_name(bundle_root.tag) != "Wix" or not bundle_root.tag.startswith("{" + WIX_NS + "}"):
                self.fail("Bundle.wxs root must use the v4 Wix namespace")
            namespace_bindings = dict(
                value for _, value in ET.iterparse(self.bundle_path, events=("start-ns",))
            )
            if namespace_bindings.get("bal") != BAL_NS:
                self.fail("Bundle.wxs is missing the v4 bal namespace")
            if namespace_bindings.get("util") != UTIL_NS:
                self.fail("Bundle.wxs is missing the v4 util namespace")
            for node in bundle_root.iter():
                namespace = node.tag[1:].partition("}")[0] if node.tag.startswith("{") else ""
                if namespace not in {WIX_NS, BAL_NS, UTIL_NS}:
                    self.fail(f"Bundle.wxs element {node.tag} has an unexpected namespace")
            body = strip_comments(self.bundle_path.read_text(encoding="utf-8"))
            if not re.search(r"<\?include\s+Product\.wxi\s*\?>", body):
                self.fail("Bundle.wxs must include Product.wxi")
        if product_root is not None and (
            local_name(product_root.tag) != "Include"
            or not product_root.tag.startswith("{" + WIX_NS + "}")
        ):
            self.fail("Product.wxi root must use the v4 Wix namespace")
        return bundle_root, product_root

    def check_ascii(self) -> None:
        for path in (self.bundle_path, self.product_path):
            if not path.is_file():
                continue
            try:
                body = strip_comments(path.read_text(encoding="utf-8"))
                body.encode("ascii")
            except UnicodeEncodeError as exc:
                self.fail(f"{path.name} contains non-ASCII content: {exc}")
            except UnicodeDecodeError as exc:
                self.fail(f"{path.name} is not UTF-8: {exc}")

    def check_variables(self, bundle_root: ET.Element | None) -> None:
        if not self.bundle_path.is_file() or not self.product_path.is_file():
            return
        product_text = self.product_path.read_text(encoding="utf-8")
        defines = set(re.findall(r'<\?define\s+([A-Za-z_][\w.]*)\s*=\s*"[^"]*"\s*\?>', product_text))
        bundle_text = strip_comments(self.bundle_path.read_text(encoding="utf-8"))
        references = set(re.findall(r"\$\(var\.([A-Za-z_][\w.]*)\)", bundle_text))
        unknown = references - defines - ALLOWED_BUILD_VARS
        if unknown:
            self.fail("Bundle.wxs references undefined variables: " + ", ".join(sorted(unknown)))
        if bundle_root is not None:
            bundle = next((node for node in bundle_root.iter() if local_name(node.tag) == "Bundle"), None)
            upgrade = attr(bundle, "UpgradeCode") if bundle is not None else None
            if upgrade != "$(var.BundleUpgradeCode)":
                self.fail("Bundle UpgradeCode must use $(var.BundleUpgradeCode)")
            expected = {
                "Name": "$(var.ProductName)", "Manufacturer": "$(var.Manufacturer)",
                "Version": "$(var.Version)", "DisableModify": "yes",
            }
            for name, value in expected.items():
                if bundle is None or bundle.get(name) != value:
                    self.fail(f"Bundle {name} must use {value}")
            ba = bundle_root.find(
                f"{{{WIX_NS}}}Bundle/{{{WIX_NS}}}BootstrapperApplication/"
                f"{{{BAL_NS}}}WixStandardBootstrapperApplication"
            )
            if ba is None or ba.get("Theme") != "hyperlinkLicense" or ba.get("LicenseUrl") != "":
                self.fail("Bundle must use hyperlinkLicense with an empty LicenseUrl")

    def check_passthrough(self, bundle_root: ET.Element | None) -> None:
        if bundle_root is None:
            return
        variables = [
            node.get("Name")
            for node in bundle_root.iter()
            if node.tag == f"{{{WIX_NS}}}Variable" and attr(node, "Overridable", BAL_NS) == "yes"
        ]
        property_nodes = [
            node
            for node in bundle_root.iter()
            if node.tag == f"{{{WIX_NS}}}MsiProperty"
        ]
        properties = {node.get("Name"): node.get("Value") for node in property_nodes}
        if set(variables) != PASS_THROUGH or len(variables) != 4:
            self.fail(f"Burn overridable variables must be exactly {sorted(PASS_THROUGH)}")
        if set(properties) != PASS_THROUGH or len(property_nodes) != 4:
            self.fail(f"MSI properties must be exactly {sorted(PASS_THROUGH)}")
        for name, value in properties.items():
            if value != f"[{name}]":
                self.fail(f"MsiProperty {name} must have value [{name}]")

        if not self.remote_path.is_file():
            self.warn("RemoteHost.wxs is absent; Secure=yes property checks require the installer source")
            return
        remote = self.parse_xml(self.remote_path)
        if remote is None:
            return
        secure_properties = {
            node.get("Id") for node in remote.iter(f"{{{WIX_NS}}}Property")
            if node.get("Secure") == "yes"
        }
        for name in sorted(PASS_THROUGH):
            if name not in secure_properties:
                self.fail(f"RemoteHost.wxs property {name} is not Secure=\"yes\"")

    def check_webview_and_chain(self, bundle_root: ET.Element | None) -> None:
        if bundle_root is None:
            return
        searches = list(bundle_root.iter(f"{{{UTIL_NS}}}RegistrySearch"))
        search = next(
            (
                node
                for node in searches
                if "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" in (node.get("Key") or "")
                and node.get("Value") == "pv"
            ),
            None,
        )
        if search is None:
            self.fail("WebView2 RegistrySearch must use the EdgeUpdate GUID and value pv")
        elif search.get("Root") != "HKLM" or search.get("Bitness") != "always32":
            self.fail("WebView2 RegistrySearch must use HKLM and Bitness=always32")
        search_variable = search.get("Variable") if search is not None else None
        chains = [node for node in bundle_root.iter() if local_name(node.tag) == "Chain"]
        chain = chains[0] if chains else None
        if chain is None:
            self.fail("Bundle is missing a Chain")
            return
        packages = list(chain)
        if len(packages) != 2 or packages[0].get("Id") != "WebView2Runtime" or packages[1].get("Id") != "RemoteHostMsi":
            self.fail("Burn Chain must place WebView2 ExePackage before the MSI")
        exe = next((node for node in packages if local_name(node.tag) == "ExePackage" and node.get("Id") == "WebView2Runtime"), None)
        if exe is None:
            self.fail("WebView2Runtime ExePackage is missing")
        else:
            if exe.get("Permanent") != "yes":
                self.fail("WebView2Runtime must be Permanent=\"yes\"")
            if exe.get("Vital") != "no":
                self.fail("WebView2Runtime must be Vital=\"no\"")
            expected = {
                "SourceFile": "$(var.WebView2Bootstrapper)", "InstallArguments": "/silent /install",
                "Compressed": "yes",
            }
            for name, value in expected.items():
                if exe.get(name) != value:
                    self.fail(f"WebView2Runtime {name} must be {value}")
            detect = exe.get("DetectCondition") or ""
            if not search_variable or not re.search(r"\b" + re.escape(search_variable) + r"\b", detect):
                self.fail("WebView2 DetectCondition must reference the RegistrySearch variable")
            if '<> "0.0.0.0"' not in detect:
                self.fail("WebView2 DetectCondition must exclude version 0.0.0.0")
        msi = next((node for node in packages if local_name(node.tag) == "MsiPackage" and node.get("Id") == "RemoteHostMsi"), None)
        if msi is None or msi.get("Vital") != "yes":
            self.fail("RemoteHostMsi must be a Vital=\"yes\" MsiPackage")
        if msi is None or msi.get("SourceFile") != "$(var.MsiPath)":
            self.fail("RemoteHostMsi must use MsiPath")

    def check_script_text(self) -> None:
        for path in (self.script_path, self.sign_path):
            if not path.is_file():
                continue
            text, code = powershell_views(path.read_text(encoding="utf-8"))
            if not re.search(r"\bSet-StrictMode\s+-Version\s+Latest\b", code):
                self.fail(f"{path.name} must set StrictMode Latest")
            if not re.search(r"\$ErrorActionPreference\s*=\s*'Stop'", text):
                self.fail(f"{path.name} must set ErrorActionPreference to Stop")
            if "??" in code or "?." in code:
                self.fail(f"{path.name} contains PowerShell 7-only null operators")
        if not self.script_path.is_file():
            return
        text, code = powershell_views(self.script_path.read_text(encoding="utf-8"))
        parameter_match = re.search(r"param\s*\((.*?)\n\)", code, flags=re.DOTALL)
        parameter_text = parameter_match.group(1) if parameter_match else ""
        for parameter in sorted(REQUIRED_PARAMETERS):
            if not re.search(r"\$" + re.escape(parameter) + r"\b", parameter_text):
                self.fail(f"win.ps1 param block is missing -{parameter}")
        for required in ("+crt-static", "--locked", "--frozen-lockfile", '"externalBin":[]', "SkipServiceStart=1"):
            if required not in text:
                self.fail(f"win.ps1 is missing required token {required}")
        detach = re.search(r"\bwix\s+burn\s+detach\b", code)
        reattach = re.search(r"\bwix\s+burn\s+reattach\b", code)
        engine_sign = re.search(r"Invoke-SignFiles\b[^\n]*-Files\s+@\(\s*\$enginePath\s*\)", code)
        setup_signs = list(re.finditer(r"Invoke-SignFiles\b[^\n]*-Files\s+@\(\s*\$setupPath\s*\)", code))
        if not (detach and reattach and detach.start() < reattach.start()):
            self.fail("win.ps1 must detach the Burn engine before reattaching it")
        if not (detach and engine_sign and reattach and detach.start() < engine_sign.start() < reattach.start()):
            self.fail("win.ps1 must sign the engine between detach and reattach")
        if not (reattach and setup_signs and reattach.start() < setup_signs[-1].start()):
            self.fail("win.ps1 must reattach the Burn engine before signing the setup executable")

        release_line = re.search(r"if\s*\(\s*-not\s+\$SkipServiceStart\s*\)\s*\{", code)
        if release_line is None:
            self.fail("win.ps1 must guard release.json generation with -not $SkipServiceStart")
        else:
            block_start = release_line.start()
            block_end = closing_brace(code, code.find("{", block_start))
            if block_end is None:
                self.fail("win.ps1 release guard has unbalanced braces")
            else:
                guarded = text[block_start:block_end]
                outside = text[:block_start] + text[block_end:]
                if "release.json" not in guarded:
                    self.fail("win.ps1 must write release.json inside the SkipServiceStart guard")
                # Deleting a stale release.json for a test build is allowed.
                # Every write to the literal path or a variable assigned that
                # path must be inside this guard.
                release_variables = set(re.findall(r"(\$\w+)\s*=[^\n]*['\"]release\.json['\"]", text))
                targets = r"['\"]release\.json['\"]"
                if release_variables:
                    targets += "|" + "|".join(re.escape(name) + r"\b" for name in release_variables)
                target = re.compile(targets)
                writer = re.compile(
                    r"\b(?:Write-AtomicJson|Set-Content|Add-Content|Out-File|Move-Item|Copy-Item|New-Item)\b"
                    r"|::(?:WriteAllText|WriteAllBytes|Replace|Move)\s*\(", re.IGNORECASE
                )
                if not any(writer.search(line) and target.search(line) for line in guarded.splitlines()):
                    self.fail("win.ps1 must write release.json inside the SkipServiceStart guard")
                if any(writer.search(line) and target.search(line) for line in outside.splitlines()):
                    self.fail("win.ps1 writes release.json outside the SkipServiceStart guard")

    def check_secrets(self) -> None:
        paths = [
            self.bundle_path, self.product_path, self.script_path, self.sign_path,
            self.checker_path,
        ]
        if self.remote_path.is_file():
            paths.append(self.remote_path)
        private_address = re.compile(
            r"(?<![\w.])(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|"
            r"172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})(?![\w.])"
        )
        fingerprint = re.compile(r"(?<![A-Fa-f0-9])[A-Fa-f0-9]{40}(?![A-Fa-f0-9])")
        for path in paths:
            if not path.is_file():
                continue
            source = path.read_text(encoding="utf-8")
            if path.suffix == ".ps1":
                body = powershell_views(source)[0]
            elif path.suffix == ".py":
                try:
                    body = " ".join(
                        token.string for token in tokenize.generate_tokens(io.StringIO(source).readline)
                        if token.type != tokenize.COMMENT
                    )
                except tokenize.TokenError as exc:
                    self.fail(f"Could not scan {path.name}: {exc}")
                    continue
            else:
                body = strip_comments(source)
            if fingerprint.search(body):
                self.fail(f"{path.name} contains a 40-character certificate fingerprint")
            if re.search(r"BEGIN\s+.*PRIVATE KEY", body, flags=re.IGNORECASE):
                self.fail(f"{path.name} contains a private-key marker")
            if re.search(r"/p\s+", body, flags=re.IGNORECASE):
                self.fail(f"{path.name} contains a signing credential parameter")
            if re.search("pass" + "word", body, flags=re.IGNORECASE):
                self.fail(f"{path.name} contains a forbidden credential word")
            if private_address.search(body):
                self.fail(f"{path.name} contains a private network address")

    def check_pwsh(self) -> None:
        if not self.optional_pwsh:
            return
        pwsh = shutil.which("pwsh")
        if pwsh is None:
            self.warn("pwsh is not installed; PowerShell syntax checks were skipped")
            return
        for path in (self.script_path, self.sign_path):
            escaped_path = str(path).replace("'", "''")
            command = (
                "$tokens = $null; $e = $null; "
                f"$null = [System.Management.Automation.Language.Parser]::ParseFile('{escaped_path}', "
                "[ref]$tokens, [ref]$e); if ($e) { $e; exit 1 }"
            )
            try:
                result = subprocess.run(
                    [pwsh, "-NoProfile", "-NonInteractive", "-Command", command],
                    text=True, capture_output=True, timeout=30,
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                self.fail(f"Could not parse {path.name} with pwsh: {exc}")
                continue
            if result.returncode != 0:
                detail = (result.stdout + result.stderr).strip().replace("\n", " ")
                self.fail(f"PowerShell parser rejected {path.name}: {detail}")

    def run(self) -> tuple[list[str], list[str]]:
        bundle_root, _ = self.check_files_and_xml()
        self.check_ascii()
        self.check_variables(bundle_root)
        self.check_passthrough(bundle_root)
        self.check_webview_and_chain(bundle_root)
        self.check_script_text()
        self.check_secrets()
        self.check_pwsh()
        if self.emit:
            for warning in self.warnings:
                print(f"WARN: {warning}")
            for error in self.errors:
                print(f"FAIL: {error}")
        return self.errors, self.warnings


def copy_check_fixture(source_repo: Path, destination: Path) -> None:
    for relative in (
        Path("tools/release/win.ps1"),
        Path("tools/release/win/Bundle.wxs"),
        Path("tools/release/win/Product.wxi"),
        Path("tools/release/win/sign.ps1"),
        Path("tools/release/win/check_bundle.py"),
    ):
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((source_repo / relative).read_bytes())
    remote = source_repo / "tools/release/win/RemoteHost.wxs"
    if remote.is_file():
        target = destination / "tools/release/win/RemoteHost.wxs"
        target.write_bytes(remote.read_bytes())


def run_self_test(repo: Path) -> bool:
    # Every mutation must fail for its intended reason; do not accept an
    # unrelated XML parse failure as evidence that the semantic check works.
    cases = [
        ("Bundle upgrade code", "tools/release/win/Bundle.wxs", "$(var.BundleUpgradeCode)", "$(var.UpgradeCode)", "Bundle UpgradeCode"),
        ("missing MSI property", "tools/release/win/Bundle.wxs", '<MsiProperty Name="PAIRFILE_LABEL" Value="[PAIRFILE_LABEL]" />', "", "MSI properties must be exactly"),
        ("WebView2 Vital", "tools/release/win/Bundle.wxs", 'Vital="no"', 'Vital="yes"', 'WebView2Runtime must be Vital="no"'),
        ("static CRT", "tools/release/win.ps1", "+crt-static", "", "missing required token +crt-static"),
        ("Burn order", "tools/release/win.ps1", None, None, "detach the Burn engine before reattaching"),
        ("non-ASCII attribute", "tools/release/win/Bundle.wxs", 'Name="$(var.ProductName)"', 'Name="\u6d4b\u8bd5"', "non-ASCII"),
        ("unguarded manifest", "tools/release/win.ps1", "    $gitCommit = $null", "    Write-AtomicJson -Path $releasePath -Value @{}\n    $gitCommit = $null", "writes release.json outside"),
    ]
    for relative in (
        "tools/release/win/Bundle.wxs", "tools/release/win/Product.wxi",
        "tools/release/win.ps1", "tools/release/win/sign.ps1",
        "tools/release/win/check_bundle.py",
    ):
        # Construct the synthetic fingerprint at runtime; no fingerprint is
        # embedded in any source file.
        synthetic = "0123456789" * 4
        if relative.endswith(".ps1"):
            marker = "\n$env:RS_WIN_SIGN_THUMBPRINT = '" + synthetic + "'\n"
        elif relative.endswith(".py"):
            marker = "\nRS_WIN_SIGN_THUMBPRINT = '" + synthetic + "'\n"
        else:
            marker = '\n<?define RS_WIN_SIGN_THUMBPRINT = "' + synthetic + '" ?>\n'
        cases.append(("fingerprint in " + relative, relative, "", marker, "certificate fingerprint"))

    temp_base = Path(os.environ.get("TMPDIR") or repo / "target" / "win-bundle-check")
    temp_base.mkdir(parents=True, exist_ok=True)
    success = True
    for label, relative, old, new, expected in cases:
        with tempfile.TemporaryDirectory(prefix="win-check-", dir=temp_base) as temporary:
            fixture = Path(temporary)
            copy_check_fixture(repo, fixture)
            path = fixture / relative
            source = path.read_text(encoding="utf-8")
            if label == "Burn order":
                mutated = source.replace("wix burn detach", "BURN_DETACH_PLACEHOLDER", 1)
                mutated = mutated.replace("wix burn reattach", "wix burn detach", 1)
                mutated = mutated.replace("BURN_DETACH_PLACEHOLDER", "wix burn reattach", 1)
            elif old == "":
                mutated = source + new
            else:
                mutated = source.replace(old, new, 1)
            if mutated == source:
                print(f"FAIL: self-test could not apply mutation: {label}")
                success = False
                continue
            path.write_text(mutated, encoding="utf-8")
            errors, _ = Checks(fixture, emit=False, optional_pwsh=False).run()
            if not any(expected in error for error in errors):
                print(f"FAIL: self-test did not detect {label}: {errors}")
                success = False
    return success


def main(argv: Iterable[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(list(argv))
    repo = args.repo.resolve()
    errors, _ = Checks(repo).run()
    if errors:
        return 1
    if args.self_test and not run_self_test(repo):
        return 1
    if args.self_test:
        print("SELF-TEST OK")
    else:
        print("OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
