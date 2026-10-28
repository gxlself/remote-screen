//! Windows Media Foundation and Video Processor probe.
//!
//! The encoder loop is intentionally kept small: it is a diagnostics helper,
//! while the production pipeline owns frame scheduling and output transport.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("mf_probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use host_win::vpmf_model::VpKey;
    use host_win::{mf_enc, vproc};
    use rsp_core::pb::Codec;
    use std::{fs::File, io::Write, str::FromStr, time::Instant};
    use windows::Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11DeviceContext, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
        },
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
    };

    unsafe {
        // SAFETY: this is the single COM initialization on the probe thread.
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }
    mf_enc::mf_startup()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().any(|arg| arg == "--list");
    let codec = value(&args, "--codec")
        .as_deref()
        .map(parse_codec)
        .transpose()?
        .unwrap_or(Codec::Hevc);
    if list {
        for adapter in vproc::adapters()? {
            println!(
                "adapter {} {:?} vendor={:#x} luid={} output={}",
                adapter.index,
                adapter.description,
                adapter.vendor_id,
                adapter.luid,
                adapter.has_output
            );
            println!(
                "  h264: {:?}",
                mf_enc::list_encoders(Some(adapter.luid), Codec::H264)?
            );
            println!(
                "  hevc: {:?}",
                mf_enc::list_encoders(Some(adapter.luid), Codec::Hevc)?
            );
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--vp-test") {
        return run_vp_test(&args);
    }
    let adapter_index = value(&args, "--adapter")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let adapters = vproc::adapters()?;
    let adapter = match adapter_index {
        Some(index) => adapters
            .iter()
            .find(|adapter| adapter.index == index)
            .ok_or("requested adapter not found")?,
        None => adapters
            .iter()
            .find(|adapter| adapter.has_output)
            .ok_or("no hardware adapter with an attached output")?,
    };
    let width = value(&args, "--size")
        .map(|size| parse_size(&size))
        .transpose()?
        .unwrap_or((1920, 1080));
    let fps = value(&args, "--fps")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(60);
    let kbps = value(&args, "--kbps")
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(10_000);
    let device = vproc::create_device(adapter.luid)?;
    let mut encoder = mf_enc::MfEncoder::open(&mf_enc::MfOpen {
        device: &device,
        adapter_luid: adapter.luid,
        vendor_id: adapter.vendor_id,
        codec,
        width: width.0,
        height: width.1,
        fps,
        bitrate_bps: kbps.saturating_mul(1000),
    })?;
    let context: ID3D11DeviceContext = unsafe {
        // SAFETY: the device is live and returns its owned immediate context.
        device.GetImmediateContext()?
    };
    let source_desc = D3D11_TEXTURE2D_DESC {
        Width: width.0,
        Height: width.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut source = None;
    unsafe {
        // SAFETY: descriptor is fully initialized and the output pointer is writable.
        device.CreateTexture2D(&source_desc, None, Some(&mut source))?;
    }
    let source = source.ok_or("CreateTexture2D returned no source texture")?;
    let mut processor = vproc::VideoProc::new(
        &device,
        VpKey {
            device_gen: 0,
            in_w: width.0,
            in_h: width.1,
            out_w: width.0,
            out_h: width.1,
            rotation: 0,
        },
        fps,
    )?;
    let frames = value(&args, "--frames")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(600);
    let idr_every = value(&args, "--idr-every")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(0);
    let rate_step = value(&args, "--rate-step")
        .map(|value| parse_rate_step(&value))
        .transpose()?;
    let bump_test = args.iter().any(|arg| arg == "--bump-test");
    let pattern = value(&args, "--pattern").unwrap_or_else(|| "bar".into());
    let mut output = value(&args, "--out").map(File::create).transpose()?;
    let mut keyframes = 0u32;
    let mut busy = 0u32;
    let mut total_bytes = 0u64;
    let mut elapsed_us = Vec::with_capacity(frames as usize);
    let mut bump_sizes = Vec::new();
    println!(
        "opened {} codec={codec:?} {}x{} fps={} kbps={kbps}",
        encoder.name(),
        width.0,
        width.1,
        fps
    );
    for frame in 0..frames {
        if let Some((step_kbps, step_frame)) = rate_step {
            if frame == step_frame {
                encoder.set_bitrate(step_kbps.saturating_mul(1000))?;
            }
        }
        let started = Instant::now();
        let pattern_frame = if bump_test { frame.min(119) } else { frame };
        let bgra = pattern_bgra(width.0, width.1, pattern_frame, &pattern);
        unsafe {
            // SAFETY: source is a live DEFAULT texture; `bgra` remains pinned for the call.
            context.UpdateSubresource(
                &source,
                0,
                None,
                bgra.as_ptr().cast(),
                width.0.saturating_mul(4),
                0,
            );
        }
        let Some((slot, destination)) = encoder.input_slot() else {
            let _ = encoder.poll()?;
            busy += 1;
            continue;
        };
        processor.blit(&source, &destination, 0)?;
        let idr = frame == 0 || (idr_every != 0 && frame % idr_every == 0);
        let bump = if bump_test && frame == 130 {
            Some(kbps.saturating_mul(2000))
        } else {
            None
        };
        let packets = encoder.encode(slot, i64::from(frame), idr, bump)?;
        let mut frame_bytes = 0u64;
        for packet in packets {
            frame_bytes += packet.data.len() as u64;
            total_bytes += packet.data.len() as u64;
            keyframes += u32::from(packet.keyframe);
            if let Some(file) = output.as_mut() {
                file.write_all(&packet.data)?;
            }
        }
        if bump_test && (frame == 129 || frame == 130) {
            bump_sizes.push(frame_bytes);
        }
        elapsed_us.push(started.elapsed().as_micros() as u64);
        if frame % fps.max(1) == fps.max(1) - 1 {
            let mut values = elapsed_us.clone();
            let p50 = percentile(&mut values, 50);
            let p95 = percentile(&mut values, 95);
            println!(
                "frames={} out_kbps={} keyframes={} busy={} enc_p50_us={} enc_p95_us={}",
                frame + 1,
                total_bytes.saturating_mul(8).saturating_mul(u64::from(fps))
                    / 1000
                    / u64::from(frame + 1),
                keyframes,
                busy,
                p50,
                p95,
            );
        }
    }
    if bump_sizes.len() == 2 && bump_sizes[0] != 0 {
        println!(
            "bump bytes={} -> {} ratio={:.2}",
            bump_sizes[0],
            bump_sizes[1],
            bump_sizes[1] as f64 / bump_sizes[0] as f64
        );
    }
    fn value(args: &[String], key: &str) -> Option<String> {
        args.windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1].clone())
    }
    fn parse_codec(value: &str) -> Result<Codec, Box<dyn std::error::Error>> {
        match value.to_ascii_lowercase().as_str() {
            "h264" => Ok(Codec::H264),
            "hevc" => Ok(Codec::Hevc),
            _ => Err(format!("unknown codec {value}").into()),
        }
    }
    fn parse_size(value: &str) -> Result<(u32, u32), Box<dyn std::error::Error>> {
        let (width, height) = value.split_once('x').ok_or("size must be WxH")?;
        Ok((u32::from_str(width)?, u32::from_str(height)?))
    }
    fn parse_rate_step(value: &str) -> Result<(u64, u32), Box<dyn std::error::Error>> {
        let (kbps, frame) = value
            .split_once('@')
            .ok_or("rate-step must be kbps@frame")?;
        Ok((kbps.parse()?, frame.parse()?))
    }
    fn pattern_bgra(width: u32, height: u32, frame: u32, pattern: &str) -> Vec<u8> {
        let mut data = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let index = (y as usize * width as usize + x as usize) * 4;
                let bar = ((x / 32 + frame / 2) % 8) as u8;
                let value = if pattern.eq_ignore_ascii_case("noise") {
                    ((x.wrapping_mul(17) ^ y.wrapping_mul(31) ^ frame.wrapping_mul(13)) & 0xff)
                        as u8
                } else {
                    bar.saturating_mul(32)
                };
                data[index] = value;
                data[index + 1] = value.wrapping_add(83);
                data[index + 2] = value.wrapping_add(167);
                data[index + 3] = 255;
            }
        }
        data
    }
    fn percentile(values: &mut [u64], percentile: u32) -> u64 {
        if values.is_empty() {
            return 0;
        }
        values.sort_unstable();
        let index = ((values.len() - 1) * percentile as usize / 100).min(values.len() - 1);
        values[index]
    }
    Ok(())
}

#[cfg(windows)]
fn run_vp_test(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use host_win::vpmf_model::{fit_rect, rotated_size, VpKey};
    use host_win::vproc;
    use std::{fs, path::Path};
    use windows::Win32::Graphics::{
        Direct3D11::{
            ID3D11DeviceContext, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
            D3D11_CPU_ACCESS_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        },
        Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
    };

    let output_size = value(args, "--size")
        .map(|size| parse_size(&size))
        .transpose()?
        .unwrap_or((1920, 1080));
    let input_size = value(args, "--in-size")
        .map(|size| parse_size(&size))
        .transpose()?
        .unwrap_or(output_size);
    let rotation = value(args, "--rotate")
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(0);
    if !matches!(rotation, 0 | 90 | 180 | 270) {
        return Err("rotate must be 0, 90, 180, or 270".into());
    }
    if input_size.0 == 0
        || input_size.1 == 0
        || output_size.0 < 2
        || output_size.1 < 2
        || output_size.0 % 2 != 0
        || output_size.1 % 2 != 0
    {
        return Err(
            "input and output sizes must be nonzero; NV12 output dimensions must be even".into(),
        );
    }
    let dump = value(args, "--dump").unwrap_or_else(|| "vp-dump".into());
    fs::create_dir_all(&dump)?;

    let adapter_index = value(args, "--adapter")
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let adapters = vproc::adapters()?;
    let adapter = adapter_index
        .and_then(|index| adapters.iter().find(|item| item.index == index))
        .or_else(|| adapters.iter().find(|item| item.has_output))
        .ok_or("no hardware adapter with an attached output")?;
    let device = vproc::create_device(adapter.luid)?;
    let context: ID3D11DeviceContext = unsafe {
        // SAFETY: the device is live and returns its owned immediate context.
        device.GetImmediateContext()?
    };

    let source_desc = D3D11_TEXTURE2D_DESC {
        Width: input_size.0,
        Height: input_size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let source = create_texture(&device, &source_desc)?;

    let output_desc = D3D11_TEXTURE2D_DESC {
        Width: output_size.0,
        Height: output_size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let output = create_texture(&device, &output_desc)?;
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: output_size.0,
        Height: output_size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let staging = create_texture(&device, &staging_desc)?;

    let input_bgra = vp_pattern(input_size.0, input_size.1);
    unsafe {
        // SAFETY: `source` is a live DEFAULT texture and the vector remains alive for the call.
        context.UpdateSubresource(
            &source,
            0,
            None,
            input_bgra.as_ptr().cast(),
            input_size.0.saturating_mul(4),
            0,
        );
    }
    write_png(
        Path::new(&dump).join("in.png"),
        input_size.0,
        input_size.1,
        &bgra_to_rgba(&input_bgra),
    )?;

    let mut processor = vproc::VideoProc::new(
        &device,
        VpKey {
            device_gen: 0,
            in_w: input_size.0,
            in_h: input_size.1,
            out_w: output_size.0,
            out_h: output_size.1,
            rotation,
        },
        60,
    )?;
    processor.blit(&source, &output, 0)?;
    unsafe {
        // SAFETY: both resources are live NV12 textures with matching dimensions and format.
        context.CopyResource(&staging, &output);
    }
    let (y_plane, uv_plane, row_pitch) =
        map_nv12(&context, &staging, output_size.0, output_size.1)?;
    let rgba = nv12_to_rgba(&y_plane, &uv_plane, output_size.0, output_size.1);
    write_png(
        Path::new(&dump).join(format!("out-{rotation}.png")),
        output_size.0,
        output_size.1,
        &rgba,
    )?;

    let effective_rotation = if processor.rotation_supported() {
        rotation
    } else {
        0
    };
    let (rotated_w, rotated_h) = rotated_size(input_size.0, input_size.1, effective_rotation);
    let target = fit_rect(rotated_w, rotated_h, output_size.0, output_size.1);
    let quadrants = [
        (input_size.0 / 4, input_size.1 / 4),
        (input_size.0.saturating_mul(3) / 4, input_size.1 / 4),
        (input_size.0 / 4, input_size.1.saturating_mul(3) / 4),
        (
            input_size.0.saturating_mul(3) / 4,
            input_size.1.saturating_mul(3) / 4,
        ),
    ];
    let labels = ["tl", "tr", "bl", "br"];
    for (label, &(x, y)) in labels.iter().zip(quadrants.iter()) {
        let (rx, ry) = rotate_point(x, y, input_size.0, input_size.1, effective_rotation);
        let ox = target.left.saturating_add(
            (u64::from(rx) * u64::from(target.right.saturating_sub(target.left))
                / u64::from(rotated_w.max(1))) as u32,
        );
        let oy = target.top.saturating_add(
            (u64::from(ry) * u64::from(target.bottom.saturating_sub(target.top))
                / u64::from(rotated_h.max(1))) as u32,
        );
        let (r, g, b) = rgba_at(&rgba, output_size.0, ox, oy);
        println!("{label}_center_rgb={r},{g},{b}");
    }
    let black = if target.left > 0 {
        (0, output_size.1 / 2)
    } else if target.top > 0 {
        (output_size.0 / 2, 0)
    } else {
        // With no letterbox, sample the test pattern's black bar instead.
        let (rx, ry) = rotate_point(
            input_size.0 / 2,
            input_size.1 / 20,
            input_size.0,
            input_size.1,
            effective_rotation,
        );
        (
            target.left.saturating_add(
                (u64::from(rx) * u64::from(target.right - target.left)
                    / u64::from(rotated_w.max(1))) as u32,
            ),
            target.top.saturating_add(
                (u64::from(ry) * u64::from(target.bottom - target.top)
                    / u64::from(rotated_h.max(1))) as u32,
            ),
        )
    };
    let black_yuv = yuv_at(
        &y_plane,
        &uv_plane,
        output_size.0,
        output_size.1,
        black.0,
        black.1,
    );
    println!(
        "black_ycbcr={} ,{},{}",
        black_yuv.0, black_yuv.1, black_yuv.2
    );
    let white = quadrants[3];
    let (rx, ry) = rotate_point(
        white.0,
        white.1,
        input_size.0,
        input_size.1,
        effective_rotation,
    );
    let white_x = target.left.saturating_add(
        (u64::from(rx) * u64::from(target.right - target.left) / u64::from(rotated_w.max(1)))
            as u32,
    );
    let white_y = target.top.saturating_add(
        (u64::from(ry) * u64::from(target.bottom - target.top) / u64::from(rotated_h.max(1)))
            as u32,
    );
    println!(
        "white_y={}",
        yuv_at(
            &y_plane,
            &uv_plane,
            output_size.0,
            output_size.1,
            white_x,
            white_y,
        )
        .0
    );
    println!(
        "vp_test adapter={} input={}x{} output={}x{} rotation={} rotation_supported={} dump={}",
        adapter.index,
        input_size.0,
        input_size.1,
        output_size.0,
        output_size.1,
        rotation,
        processor.rotation_supported(),
        dump
    );
    let _ = row_pitch;
    Ok(())
}

#[cfg(windows)]
fn value(args: &[String], key: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1].clone())
}

#[cfg(windows)]
fn parse_size(value: &str) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    use std::str::FromStr;
    let (width, height) = value.split_once('x').ok_or("size must be WxH")?;
    Ok((u32::from_str(width)?, u32::from_str(height)?))
}

#[cfg(windows)]
fn create_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    desc: &windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC,
) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, Box<dyn std::error::Error>> {
    let mut texture = None;
    unsafe {
        // SAFETY: descriptor is fully initialized and the output pointer is writable.
        device.CreateTexture2D(desc, None, Some(&mut texture))?;
    }
    texture.ok_or_else(|| "CreateTexture2D returned no texture".into())
}

#[cfg(windows)]
fn vp_pattern(width: u32, height: u32) -> Vec<u8> {
    let mut data = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let (r, g, b) = if y < height / 2 {
                if x < width / 2 {
                    (255, 0, 0)
                } else {
                    (0, 255, 0)
                }
            } else if x < width / 2 {
                (0, 0, 255)
            } else {
                (255, 255, 255)
            };
            let bar = y < (height / 10).max(2) && x >= width / 4 && x < width.saturating_mul(3) / 4;
            let index = (y as usize * width as usize + x as usize) * 4;
            data[index] = if bar { 0 } else { b };
            data[index + 1] = if bar { 0 } else { g };
            data[index + 2] = if bar { 0 } else { r };
            data[index + 3] = 255;
        }
    }
    data
}

#[cfg(windows)]
fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect()
}

/// Luma plane, interleaved chroma plane, and row pitch of a mapped NV12 texture.
#[cfg(windows)]
type Nv12Planes = (Vec<u8>, Vec<u8>, u32);

#[cfg(windows)]
fn map_nv12(
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    staging: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<Nv12Planes, Box<dyn std::error::Error>> {
    use windows::Win32::Graphics::Direct3D11::{D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ};
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        // SAFETY: staging is a CPU-readable staging resource; Map completes the queued copy.
        context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    }
    let pitch = mapped.RowPitch;
    let mut y_plane = vec![0u8; width as usize * height as usize];
    let mut uv_plane = vec![0u8; width as usize * (height as usize / 2)];
    unsafe {
        // SAFETY: Map returned a valid pointer and the two NV12 planes fit within the mapped
        // row pitch; each copy stays within the mapped staging allocation.
        let base = mapped.pData.cast::<u8>();
        for row in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                base.add(row * pitch as usize),
                y_plane.as_mut_ptr().add(row * width as usize),
                width as usize,
            );
        }
        let uv_base = base.add(pitch as usize * height as usize);
        for row in 0..height as usize / 2 {
            std::ptr::copy_nonoverlapping(
                uv_base.add(row * pitch as usize),
                uv_plane.as_mut_ptr().add(row * width as usize),
                width as usize,
            );
        }
        context.Unmap(staging, 0);
    }
    Ok((y_plane, uv_plane, pitch))
}

#[cfg(windows)]
fn yuv_at(y: &[u8], uv: &[u8], width: u32, height: u32, x: u32, row: u32) -> (u8, u8, u8) {
    let x = x.min(width.saturating_sub(1));
    let row = row.min(height.saturating_sub(1));
    let y_value = y[row as usize * width as usize + x as usize];
    let uv_index = (row as usize / 2) * width as usize + (x as usize / 2) * 2;
    (y_value, uv[uv_index], uv[uv_index + 1])
}

#[cfg(windows)]
fn nv12_to_rgba(y: &[u8], uv: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height {
        for x in 0..width {
            let (yv, cb, cr) = yuv_at(y, uv, width, height, x, row);
            let luma = f32::from(yv) - 16.0;
            let cb = f32::from(cb) - 128.0;
            let cr = f32::from(cr) - 128.0;
            let r = 1.164_383 * luma + 1.792_741 * cr;
            let g = 1.164_383 * luma - 0.213_249 * cb - 0.532_909 * cr;
            let b = 1.164_383 * luma + 2.112_402 * cb;
            let index = (row as usize * width as usize + x as usize) * 4;
            rgba[index] = r.round().clamp(0.0, 255.0) as u8;
            rgba[index + 1] = g.round().clamp(0.0, 255.0) as u8;
            rgba[index + 2] = b.round().clamp(0.0, 255.0) as u8;
            rgba[index + 3] = 255;
        }
    }
    rgba
}

#[cfg(windows)]
fn rgba_at(rgba: &[u8], width: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let index = (y as usize * width as usize + x as usize) * 4;
    (rgba[index], rgba[index + 1], rgba[index + 2])
}

#[cfg(windows)]
fn rotate_point(x: u32, y: u32, width: u32, height: u32, rotation: u32) -> (u32, u32) {
    use host_win::vpmf_model::{self, VpRotation};
    match vpmf_model::vp_rotation(rotation) {
        VpRotation::Identity => (x, y),
        VpRotation::R90 => (height.saturating_sub(1).saturating_sub(y), x),
        VpRotation::R180 => (
            width.saturating_sub(1).saturating_sub(x),
            height.saturating_sub(1).saturating_sub(y),
        ),
        VpRotation::R270 => (y, width.saturating_sub(1).saturating_sub(x)),
    }
}

#[cfg(windows)]
fn write_png(
    path: impl AsRef<std::path::Path>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::{fs::File, io::BufWriter};
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}
