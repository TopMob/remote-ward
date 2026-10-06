use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1};
use windows::Win32::System::Threading::GetCurrentProcessId;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pid = unsafe { GetCurrentProcessId() };
    println!("PID: {}", pid);

    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut adapter_idx = 0;

    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_idx) } {
        let desc = unsafe { adapter.GetDesc1()? };
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]);
        println!("\nAdapter #{}: {}", adapter_idx, name);

        let mut output_idx = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_idx) } {
            let out_desc = unsafe { output.GetDesc()? };
            let out_name = String::from_utf16_lossy(&out_desc.DeviceName);
            let out_name = out_name.trim_matches('\0');
            let coords = out_desc.DesktopCoordinates;
            let attached = out_desc.AttachedToDesktop.as_bool();
            println!(
                "  Output #{}: '{}', Attached: {}, Rect: [{}, {}, {}, {}]",
                output_idx,
                out_name,
                attached,
                coords.left,
                coords.top,
                coords.right,
                coords.bottom
            );

            // Попытка 1: Устройство через указанный адаптер
            let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let mut dev1: Option<ID3D11Device> = None;
            let mut ctx1: Option<ID3D11DeviceContext> = None;

            let res_dev1 = unsafe {
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut dev1),
                    None,
                    Some(&mut ctx1),
                )
            };
            println!("    D3D11CreateDevice(&adapter): {:?}", res_dev1);

            if let Some(dev) = dev1 {
                let out1: IDXGIOutput1 = output.cast()?;
                let dup_res = unsafe { out1.DuplicateOutput(&dev) };
                match dup_res {
                    Ok(_) => println!("    ✅ DuplicateOutput(&dev_with_adapter) УСПЕШНО!"),
                    Err(e) => println!("    ❌ DuplicateOutput(&dev_with_adapter) ОШИБКА: {:?}", e),
                }
            }

            // Попытка 2: Устройство через HARDWARE (default adapter)
            let mut dev2: Option<ID3D11Device> = None;
            let mut ctx2: Option<ID3D11DeviceContext> = None;
            let res_dev2 = unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut dev2),
                    None,
                    Some(&mut ctx2),
                )
            };
            println!("    D3D11CreateDevice(None, HARDWARE): {:?}", res_dev2);

            if let Some(dev) = dev2 {
                let out1: IDXGIOutput1 = output.cast()?;
                let dup_res = unsafe { out1.DuplicateOutput(&dev) };
                match dup_res {
                    Ok(_) => println!("    ✅ DuplicateOutput(HARDWARE) УСПЕШНО!"),
                    Err(e) => println!("    ❌ DuplicateOutput(HARDWARE) ОШИБКА: {:?}", e),
                }
            }

            output_idx += 1;
        }

        adapter_idx += 1;
    }

    Ok(())
}
