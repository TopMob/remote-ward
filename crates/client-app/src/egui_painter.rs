//! # Direct3D 11 Egui Painter
//!
//! Высокопроизводительный D3D11-бэкенд для библиотеки egui, использующий
//! аппаратное альфа-смешивание (Premultiplied Alpha), аппаратные Scissor Rects
//! с учетом DPI-масштабирования и динамические буферы вершин/индексов.

use std::collections::HashMap;
use windows::core::s;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11BlendState, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView,
    ID3D11Texture2D, ID3D11VertexShader,
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER,
    D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE,
    D3D11_BLEND_OP_ADD, D3D11_BOX, D3D11_BUFFER_DESC,
    D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_CPU_ACCESS_WRITE,
    D3D11_CULL_NONE, D3D11_FILL_SOLID, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD,
    D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC,
    D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_USAGE_DYNAMIC,
    D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32_UINT,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};

const EGUI_VS_HLSL: &str = r#"
cbuffer ScreenConstants : register(b0) {
    float2 u_screen_size;
    float2 u_padding;
};

struct VSInput {
    float2 pos : POSITION;
    float2 uv  : TEXCOORD0;
    float4 col : COLOR0;
};

struct PSInput {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
    float4 col : COLOR0;
};

PSInput main(VSInput input) {
    PSInput output;
    output.pos = float4(
        (input.pos.x / u_screen_size.x) * 2.0 - 1.0,
        1.0 - (input.pos.y / u_screen_size.y) * 2.0,
        0.0,
        1.0
    );
    output.uv  = input.uv;
    output.col = input.col;
    return output;
}
"#;

const EGUI_PS_HLSL: &str = r#"
Texture2D    font_texture : register(t0);
SamplerState font_sampler : register(s0);

struct PSInput {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
    float4 col : COLOR0;
};

float4 main(PSInput input) : SV_TARGET {
    return input.col * font_texture.Sample(font_sampler, input.uv);
}
"#;

pub struct D3D11EguiPainter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    constant_buffer: ID3D11Buffer,
    vertex_buffer: ID3D11Buffer,
    index_buffer: ID3D11Buffer,
    vb_capacity: usize,
    ib_capacity: usize,
    blend_state: ID3D11BlendState,
    rasterizer_state: ID3D11RasterizerState,
    sampler_state: ID3D11SamplerState,
    textures: HashMap<egui::TextureId, (ID3D11Texture2D, ID3D11ShaderResourceView)>,
}

impl D3D11EguiPainter {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self, windows::core::Error> {
        let (vs, vs_code) = compile_vertex_shader_with_code(device, EGUI_VS_HLSL)?;
        let ps = compile_pixel_shader(device, EGUI_PS_HLSL)?;

        // 1. Создание Input Layout для egui::epaint::Vertex
        let input_elements = [
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 8,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: s!("COLOR"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                InputSlot: 0,
                AlignedByteOffset: 16,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];

        let mut input_layout = None;
        unsafe {
            device.CreateInputLayout(
                &input_elements,
                std::slice::from_raw_parts(vs_code.GetBufferPointer() as *const u8, vs_code.GetBufferSize()),
                Some(&mut input_layout),
            )?;
        }
        let input_layout = input_layout.unwrap();

        // 2. Constant Buffer для передачи разрешения экрана (16 байт)
        let cb_desc = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut constant_buffer = None;
        unsafe {
            device.CreateBuffer(&cb_desc, None, Some(&mut constant_buffer))?;
        }
        let constant_buffer = constant_buffer.unwrap();

        // 3. Начальные динамические буферы вершин и индексов
        let initial_vb_capacity = 16384;
        let initial_ib_capacity = 32768;

        let vertex_buffer = create_dynamic_buffer(
            device,
            D3D11_BIND_VERTEX_BUFFER.0 as u32,
            initial_vb_capacity * std::mem::size_of::<egui::epaint::Vertex>(),
        )?;
        let index_buffer = create_dynamic_buffer(
            device,
            D3D11_BIND_INDEX_BUFFER.0 as u32,
            initial_ib_capacity * std::mem::size_of::<u32>(),
        )?;

        // 4. Blend State (Premultiplied Alpha)
        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: windows::Win32::Foundation::BOOL(1),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend_state = None;
        unsafe {
            device.CreateBlendState(&blend_desc, Some(&mut blend_state))?;
        }
        let blend_state = blend_state.unwrap();

        // 5. Rasterizer State со включенным Scissor Enable и без отсечения граней (Cull None)
        let raster_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            FrontCounterClockwise: windows::Win32::Foundation::BOOL(0),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: windows::Win32::Foundation::BOOL(1),
            ScissorEnable: windows::Win32::Foundation::BOOL(1),
            MultisampleEnable: windows::Win32::Foundation::BOOL(0),
            AntialiasedLineEnable: windows::Win32::Foundation::BOOL(0),
        };
        let mut rasterizer_state = None;
        unsafe {
            device.CreateRasterizerState(&raster_desc, Some(&mut rasterizer_state))?;
        }
        let rasterizer_state = rasterizer_state.unwrap();

        // 6. Sampler State для шрифтов (Linear + Clamp)
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler_state = None;
        unsafe {
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler_state))?;
        }
        let sampler_state = sampler_state.unwrap();

        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            vs,
            ps,
            input_layout,
            constant_buffer,
            vertex_buffer,
            index_buffer,
            vb_capacity: initial_vb_capacity,
            ib_capacity: initial_ib_capacity,
            blend_state,
            rasterizer_state,
            sampler_state,
            textures: HashMap::new(),
        })
    }

    /// Очистить буфер кадра заданным цветом
    pub fn clear(&self, render_target: &ID3D11RenderTargetView, color_rgba: [f32; 4]) {
        unsafe {
            self.context.ClearRenderTargetView(render_target, &color_rgba);
        }
    }

    /// Обновить текстуры egui (шрифтовой атлас и пользовательские изображения)
    pub fn update_textures(&mut self, textures_delta: &egui::TexturesDelta) -> Result<(), windows::core::Error> {
        for (id, delta) in &textures_delta.set {
            self.set_texture(*id, delta)?;
        }
        for id in &textures_delta.free {
            self.textures.remove(id);
        }
        Ok(())
    }

    fn set_texture(
        &mut self,
        id: egui::TextureId,
        delta: &egui::epaint::ImageDelta,
    ) -> Result<(), windows::core::Error> {
        let (width, height, rgba_data): (usize, usize, std::borrow::Cow<[u8]>) = match &delta.image {
            egui::ImageData::Color(image) => {
                let bytes: &[u8] = bytemuck::cast_slice(&image.pixels);
                (image.size[0], image.size[1], std::borrow::Cow::Borrowed(bytes))
            }
            egui::ImageData::Font(font) => {
                let pixels: Vec<u8> = font.srgba_pixels(None).flat_map(|c| c.to_array()).collect();
                (font.size[0], font.size[1], std::borrow::Cow::Owned(pixels))
            }
        };

        if let Some([origin_x, origin_y]) = delta.pos {
            // Частичное обновление уже существующей текстуры
            if let Some((tex, _)) = self.textures.get(&id) {
                let sub_box = D3D11_BOX {
                    left: origin_x as u32,
                    top: origin_y as u32,
                    front: 0,
                    right: (origin_x + width) as u32,
                    bottom: (origin_y + height) as u32,
                    back: 1,
                };
                unsafe {
                    self.context.UpdateSubresource(
                        tex,
                        0,
                        Some(&sub_box),
                        rgba_data.as_ptr() as *const _,
                        (width * 4) as u32,
                        0,
                    );
                }
            }
        } else {
            // Полное создание новой текстуры
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };

            let init_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: rgba_data.as_ptr() as *const _,
                SysMemPitch: (width * 4) as u32,
                SysMemSlicePitch: 0,
            };

            let mut tex = None;
            unsafe {
                self.device.CreateTexture2D(&desc, Some(&init_data), Some(&mut tex))?;
            }
            let tex = tex.unwrap();

            let mut srv = None;
            unsafe {
                self.device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
            }
            let srv = srv.unwrap();

            self.textures.insert(id, (tex, srv));
        }

        Ok(())
    }

    /// Отрисовка сгенерированных egui примитивов
    pub fn paint(
        &mut self,
        render_target: &ID3D11RenderTargetView,
        screen_width: u32,
        screen_height: u32,
        pixels_per_point: f32,
        clipped_primitives: &[egui::ClippedPrimitive],
    ) -> Result<(), windows::core::Error> {
        if screen_width == 0 || screen_height == 0 || clipped_primitives.is_empty() {
            return Ok(());
        }

        // Подсчет требуемого размера буферов
        let mut total_vertices = 0;
        let mut total_indices = 0;
        for clipped in clipped_primitives {
            if let egui::epaint::Primitive::Mesh(ref mesh) = clipped.primitive {
                total_vertices += mesh.vertices.len();
                total_indices += mesh.indices.len();
            }
        }

        if total_vertices == 0 || total_indices == 0 {
            return Ok(());
        }

        // Автоматическое расширение буферов при необходимости
        if total_vertices > self.vb_capacity {
            self.vb_capacity = (total_vertices * 2).max(16384);
            self.vertex_buffer = create_dynamic_buffer(
                &self.device,
                D3D11_BIND_VERTEX_BUFFER.0 as u32,
                self.vb_capacity * std::mem::size_of::<egui::epaint::Vertex>(),
            )?;
        }

        if total_indices > self.ib_capacity {
            self.ib_capacity = (total_indices * 2).max(32768);
            self.index_buffer = create_dynamic_buffer(
                &self.device,
                D3D11_BIND_INDEX_BUFFER.0 as u32,
                self.ib_capacity * std::mem::size_of::<u32>(),
            )?;
        }

        // 1. Копирование вершин в динамический GPU буфер
        unsafe {
            let mut mapped_vb = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&self.vertex_buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped_vb))?;
            let mut vb_dst = mapped_vb.pData as *mut egui::epaint::Vertex;
            for clipped in clipped_primitives {
                if let egui::epaint::Primitive::Mesh(ref mesh) = clipped.primitive {
                    std::ptr::copy_nonoverlapping(mesh.vertices.as_ptr(), vb_dst, mesh.vertices.len());
                    vb_dst = vb_dst.add(mesh.vertices.len());
                }
            }
            self.context.Unmap(&self.vertex_buffer, 0);

            // 2. Копирование индексов в динамический GPU буфер
            let mut mapped_ib = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&self.index_buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped_ib))?;
            let mut ib_dst = mapped_ib.pData as *mut u32;
            for clipped in clipped_primitives {
                if let egui::epaint::Primitive::Mesh(ref mesh) = clipped.primitive {
                    std::ptr::copy_nonoverlapping(mesh.indices.as_ptr(), ib_dst, mesh.indices.len());
                    ib_dst = ib_dst.add(mesh.indices.len());
                }
            }
            self.context.Unmap(&self.index_buffer, 0);

            // 3. Обновление Constant Buffer (размер экрана)
            let mut mapped_cb = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&self.constant_buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped_cb))?;
            let screen_size_data = [
                screen_width as f32 / pixels_per_point,
                screen_height as f32 / pixels_per_point,
                0.0f32,
                0.0f32,
            ];
            std::ptr::copy_nonoverlapping(screen_size_data.as_ptr(), mapped_cb.pData as *mut f32, 4);
            self.context.Unmap(&self.constant_buffer, 0);

            // 4. Настройка конвейера отрисовки Direct3D 11
            let viewport = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: screen_width as f32,
                Height: screen_height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            self.context.RSSetViewports(Some(&[viewport]));
            self.context.OMSetRenderTargets(Some(&[Some(render_target.clone())]), None);

            let blend_factors = [0.0f32, 0.0, 0.0, 0.0];
            self.context.OMSetBlendState(Some(&self.blend_state), Some(&blend_factors), 0xFFFFFFFF);
            self.context.RSSetState(Some(&self.rasterizer_state));

            self.context.IASetInputLayout(Some(&self.input_layout));
            self.context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

            let stride = std::mem::size_of::<egui::epaint::Vertex>() as u32;
            let offset = 0u32;
            self.context.IASetVertexBuffers(0, 1, Some(&Some(self.vertex_buffer.clone())), Some(&stride), Some(&offset));
            self.context.IASetIndexBuffer(Some(&self.index_buffer), DXGI_FORMAT_R32_UINT, 0);

            self.context.VSSetShader(Some(&self.vs), None);
            self.context.VSSetConstantBuffers(0, Some(&[Some(self.constant_buffer.clone())]));

            self.context.PSSetShader(Some(&self.ps), None);
            self.context.PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));

            // 5. Поэлементная отрисовка сеток с учетом Scissor Rects
            let mut vertex_offset = 0i32;
            let mut index_offset = 0u32;

            for clipped in clipped_primitives {
                if let egui::epaint::Primitive::Mesh(ref mesh) = clipped.primitive {
                    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                        continue;
                    }

                    // Переводим логические координаты egui в физические пиксели D3D11
                    let min_x = (clipped.clip_rect.min.x * pixels_per_point).round().max(0.0) as i32;
                    let min_y = (clipped.clip_rect.min.y * pixels_per_point).round().max(0.0) as i32;
                    let max_x = (clipped.clip_rect.max.x * pixels_per_point).round().min(screen_width as f32) as i32;
                    let max_y = (clipped.clip_rect.max.y * pixels_per_point).round().min(screen_height as f32) as i32;

                    if max_x > min_x && max_y > min_y {
                        let scissor = RECT {
                            left: min_x,
                            top: min_y,
                            right: max_x,
                            bottom: max_y,
                        };
                        self.context.RSSetScissorRects(Some(&[scissor]));

                        if let Some((_, srv)) = self.textures.get(&mesh.texture_id) {
                            self.context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                        }

                        self.context.DrawIndexed(mesh.indices.len() as u32, index_offset, vertex_offset);
                    }

                    vertex_offset += mesh.vertices.len() as i32;
                    index_offset += mesh.indices.len() as u32;
                }
            }

            // Сброс привязок SRV
            let null_srvs: [Option<ID3D11ShaderResourceView>; 1] = [None];
            self.context.PSSetShaderResources(0, Some(&null_srvs));
        }

        Ok(())
    }
}

fn create_dynamic_buffer(
    device: &ID3D11Device,
    bind_flags: u32,
    size_bytes: usize,
) -> Result<ID3D11Buffer, windows::core::Error> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size_bytes as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind_flags,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    let mut buffer = None;
    unsafe {
        device.CreateBuffer(&desc, None, Some(&mut buffer))?;
    }
    Ok(buffer.unwrap())
}

fn compile_vertex_shader_with_code(
    device: &ID3D11Device,
    source: &str,
) -> Result<(ID3D11VertexShader, windows::Win32::Graphics::Direct3D::ID3DBlob), windows::core::Error> {
    let mut code = None;
    let mut error = None;
    unsafe {
        let _ = D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            None,
            None,
            None,
            s!("main"),
            s!("vs_5_0"),
            0,
            0,
            &mut code,
            Some(&mut error),
        );
        let code = code.expect("Failed to compile vertex shader");
        let mut vs = None;
        device.CreateVertexShader(
            std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()),
            None,
            Some(&mut vs),
        )?;
        Ok((vs.unwrap(), code))
    }
}

fn compile_pixel_shader(
    device: &ID3D11Device,
    source: &str,
) -> Result<ID3D11PixelShader, windows::core::Error> {
    let mut code = None;
    let mut error = None;
    unsafe {
        let _ = D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            None,
            None,
            None,
            s!("main"),
            s!("ps_5_0"),
            0,
            0,
            &mut code,
            Some(&mut error),
        );
        let code = code.expect("Failed to compile pixel shader");
        let mut ps = None;
        device.CreatePixelShader(
            std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()),
            None,
            Some(&mut ps),
        )?;
        Ok(ps.unwrap())
    }
}
