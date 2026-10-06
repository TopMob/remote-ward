use std::time::Instant;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_MODE_CHANGE_IN_PROGRESS, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

use crate::error::CaptureError;

/// Информация о видеокарте
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub index: u32,
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: usize,
}

/// RAII-обёртка над захваченным кадром.
/// При выходе из области видимости автоматически освобождает кадр в DXGI.
pub struct AcquiredFrame<'a> {
    duplication: &'a IDXGIOutputDuplication,
    pub texture: ID3D11Texture2D,
    pub info: DXGI_OUTDUPL_FRAME_INFO,
    pub acquired_at: Instant,
}

impl<'a> Drop for AcquiredFrame<'a> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.duplication.ReleaseFrame();
        }
    }
}

pub struct DxgiCapturer {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub duplication: IDXGIOutputDuplication,
    pub width: u32,
    pub height: u32,
    pub adapter_index: u32,
    pub output_index: u32,
}

impl DxgiCapturer {
    /// Получить список всех доступных GPU-адаптеров
    pub fn enumerate_adapters() -> Result<Vec<AdapterInfo>, CaptureError> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let mut list = Vec::new();
        let mut index = 0;

        while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
            let desc = unsafe { adapter.GetDesc1()? };

            let len = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let description = String::from_utf16_lossy(&desc.Description[..len]);

            list.push(AdapterInfo {
                index,
                description,
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                dedicated_video_memory: desc.DedicatedVideoMemory,
            });
            index += 1;
        }

        Ok(list)
    }

    /// Инициализация захвата для указанного GPU (по умолчанию 0 или дискретная карта) и монитора (по умолчанию 0)
    pub fn new(adapter_index: u32, output_index: u32) -> Result<Self, CaptureError> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let adapter: IDXGIAdapter1 = unsafe {
            factory
                .EnumAdapters1(adapter_index)
                .map_err(|_| CaptureError::NoAdapterFound)?
        };

        let output: IDXGIOutput = unsafe {
            adapter
                .EnumOutputs(output_index)
                .map_err(|_| CaptureError::OutputNotFound(output_index))?
        };

        let output_desc = unsafe { output.GetDesc()? };
        let width = (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left)
            .unsigned_abs();
        let height = (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top)
            .unsigned_abs();

        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device_opt: Option<ID3D11Device> = None;
        let mut context_opt: Option<ID3D11DeviceContext> = None;

        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device_opt),
                None,
                Some(&mut context_opt),
            )?;
        }

        let device = device_opt.expect("D3D11 device must be created");
        let context = context_opt.expect("D3D11 immediate context must be created");

        let output1: IDXGIOutput1 = output.cast()?;
        let duplication = unsafe { output1.DuplicateOutput(&device)? };

        tracing::info!(
            "DXGI Capturer успешно создан: {}x{}, адаптер #{}, монитор #{}",
            width,
            height,
            adapter_index,
            output_index
        );

        Ok(Self {
            device,
            context,
            duplication,
            width,
            height,
            adapter_index,
            output_index,
        })
    }

    /// Попытка пересоздать дубликатор при потере контекста (DXGI_ERROR_ACCESS_LOST)
    pub fn recreate(&mut self) -> Result<(), CaptureError> {
        tracing::warn!("Пересоздание DXGI Desktop Duplication...");
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let adapter: IDXGIAdapter1 = unsafe {
            factory
                .EnumAdapters1(self.adapter_index)
                .map_err(|_| CaptureError::NoAdapterFound)?
        };
        let output: IDXGIOutput = unsafe {
            adapter
                .EnumOutputs(self.output_index)
                .map_err(|_| CaptureError::OutputNotFound(self.output_index))?
        };
        let output1: IDXGIOutput1 = output.cast()?;
        self.duplication = unsafe { output1.DuplicateOutput(&self.device)? };
        tracing::info!("DXGI Desktop Duplication успешно пересоздан");
        Ok(())
    }

    /// Захват следующего кадра с таймаутом
    pub fn acquire_frame(&self, timeout_ms: u32) -> Result<AcquiredFrame<'_>, CaptureError> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource_opt: Option<IDXGIResource> = None;

        let res = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource_opt)
        };

        if let Err(e) = res {
            let hr = e.code();
            if hr == DXGI_ERROR_WAIT_TIMEOUT {
                return Err(CaptureError::Timeout(timeout_ms));
            } else if hr == DXGI_ERROR_ACCESS_LOST {
                return Err(CaptureError::AccessLost);
            } else if hr == DXGI_ERROR_MODE_CHANGE_IN_PROGRESS {
                return Err(CaptureError::ModeChanged);
            } else {
                return Err(CaptureError::Windows(e));
            }
        }

        let resource = resource_opt.expect("Desktop resource should not be null on success");
        let texture: ID3D11Texture2D = resource.cast()?;

        Ok(AcquiredFrame {
            duplication: &self.duplication,
            texture,
            info: frame_info,
            acquired_at: Instant::now(),
        })
    }
}
