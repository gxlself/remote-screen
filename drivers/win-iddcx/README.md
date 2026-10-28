# Remote Screen Windows virtual display driver

This directory contains the UMDF 2 / IddCx driver used by Remote Screen on
Windows. It is based on Microsoft's `Windows-driver-samples/video/IndirectDisplay/IddSampleDriver`
and the adapted source remains under the MIT License:

```text
Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The driver creates virtual monitors and exposes their modes and EDID through
the control device. It does not process desktop pixels. The session process
captures the resulting monitor with DXGI Desktop Duplication and handles
encoding and transport.

## IOCTLs and lease

The wire structures and IOCTL values are defined in
[`include/RsIddIoctl.h`](include/RsIddIoctl.h). The service uses `GET_INFO`,
`ADD`, `REMOVE`, `REMOVE_ALL`, and `PING`. The service sends `PING` once per
second. Every IOCTL renews the lease, and the driver removes all virtual
monitors after five seconds without a request.

The driver supports at most four monitors. The hardware ID is
`Root\RsVirtualDisplay`; the display name is supplied by the service as
`RS Virtual NN`. The system chooses the initial desktop position.

## Build

Build on Windows with Visual Studio 2022 and a matching Windows Driver Kit, or
from an EWDK developer PowerShell. From this directory run:

```powershell
.\build.ps1
.\build.ps1 -Configuration Debug
```

The script builds `RsIdd.sln`, stamps `RsIdd.inf`, creates a catalog with
`Inf2Cat`, creates or reuses a current-user test code-signing certificate, and
signs the DLL and catalog. It writes the package to `out\RsIdd\` and the
public test certificate to `out\RsIdd-test.cer`. Add `-TimestampServer` to use
an RFC 3161 timestamp service; by default no timestamp is added.

The `-u 2.15.0` value passed to `stampinf` is the UMDF version used by the
current build script. Keep it aligned with the WDK installed on the build
machine when that WDK requires a different UMDF version.

## Install and uninstall

Run PowerShell as Administrator:

```powershell
.\install.ps1
.\uninstall.ps1
.\uninstall.ps1 -RemoveCert
```

`install.ps1` imports the public certificate into **Trusted Root Certification
Authorities** and **Trusted Publishers**, stages the package with `pnputil`,
creates the Root device if necessary, and waits up to ten seconds for it to
start. The script needs `devcon.exe`; copy the x64 binary from the WDK
`Tools\x64` directory to the test machine or put it in `PATH`.

`uninstall.ps1` removes the Root device and every installed package whose
original name is `rsidd.inf`. It is safe to run it when the device or package
has already been removed.

## Driver signing choices

1. **Internal testing:** use the self-signed certificate created by
   `build.ps1`, then import it into the two stores above.
2. **If Windows still rejects the package:** enable Windows test-signing mode
   with `bcdedit /set testsigning on` after disabling Secure Boot, then reboot.
   The install script never changes boot configuration automatically.
3. **Production release:** use an EV code-signing certificate and Microsoft
   Partner Center attestation signing. This version does not perform WHQL or
   HLK signing.

## Validation limits

- Validate on a locally logged-in console session; RDP is not supported for the
  virtual-display acceptance test.
- Up to four virtual monitors are supported.
- A mode may be at most 3840x2560 at 60 Hz.
- The probe and service use the system's default monitor placement (normally
  the right side) and the system's default DPI scaling.
- The driver is not included in an MSI yet.
