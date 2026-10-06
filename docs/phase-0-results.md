# Результаты исследований и тестов Фазы 0 (Phase 0 Spike)

> Дата: 6 октября 2026  
> Оборудование хоста: AMD Ryzen 7 5700X, NVIDIA GeForce RTX 2060 SUPER (8 ГБ VRAM), Windows 11 Pro  
> Дисплей: 2560x1440 (2K) "P27QBA-RX"  

---

## 1. Сводка результатов

| Компонент / Гипотеза | Результат | Статус |
|---|---|---|
| **Рабочее окружение** | MSVC 14.51 + Win11 SDK 26100 + Rust 1.99.0 (`x86_64-pc-windows-msvc`) | ✅ Развернуто и протестировано |
| **Бинарный протокол (`core-protocol`)** | Zero-copy заголовок датаграммы (22 байта), фрагментатор под MTU 1380 Б, события ввода со скан-кодами и относительной мышью | ✅ Реализован, 100% тестов пройдено |
| **Захват экрана на GPU (`host-capture`)** | Windows Graphics Capture (WGC) захватывает кадры 2560x1440 с частотой ~17.8 мс (~60 FPS) с прямым доступом к `ID3D11Texture2D` на GPU | ✅ Подтверждено живым бенчмарком на RTX 2060 SUPER |
| **Сетевой транспорт (`core-transport`)** | Сборщик фрагментов `FrameReassembler` с поддержкой переупорядочивания пакетов и очисткой устаревших кадров; калькулятор пейсинга `PacketPacer` | ✅ Реализован, 100% тестов пройдено |

---

## 2. Ключевые открытия по захвату экрана (DXGI vs WGC)

В ходе стресс-тестирования на Windows 11 с активными играми (CS2) и фоновыми службами:

1. **DXGI Desktop Duplication (`IDXGIOutput1::DuplicateOutput`):**
   * Возвращает ошибку `0x80070005 (E_ACCESSDENIED)`, если в системе уже есть процесс, монопольно захватывающий данный дисплей (например, фоновая запись Steam Game Recording, GeForce Experience или оверлей).
   * Вывод: DXGI сохраняем в архитектуре как оптимизированный бэкенд, но **основным и наиболее надежным для Windows 11 является Windows Graphics Capture (WGC)**.

2. **Windows Graphics Capture (WGC):**
   * Не имеет ограничений на монопольный захват — работает параллельно с любыми играми и службами.
   * Поддерживает отключение желтой рамки (`DrawBorderSettings::WithoutBorder`).
   * Кадр передаётся напрямую как Direct3D11 текстура (`ID3D11Texture2D`) в VRAM без копирования в CPU RAM (Zero-Copy).
   * Реальное время межкадрового интервала на тесте: **17.75 – 17.85 мс (стабильные 60 FPS)**.

---

## 3. Архитектура созданных крейтов

```
crates/
├── core-protocol/
│   ├── packet.rs       # DatagramHeader (22 байта, big-endian, bytemuck zero-copy)
│   ├── frame.rs        # FramePacketizer (нарезка на фрагменты под MTU), FrameMetadata
│   ├── input.rs        # MouseMoveRelative, MouseButton, Keyboard (Hardware Scan Codes), GamepadState
│   └── control.rs      # ClientHello, ServerHello, RequestKeyframe, ChangeStreamSettings
│
├── core-transport/
│   ├── reassembler.rs  # FrameReassembler (сборка кадра из датаграмм, защита от потерь)
│   └── pacer.rs        # PacketPacer (сглаживание всплесков отправки по межкадровому интервалу)
│
└── host-capture/
    ├── dxgi.rs         # DXGI Desktop Duplication модуль
    ├── examples/
    │   ├── test_wgc.rs # Бенчмарк аппаратного захвата экрана WGC (2560x1440 @ 60 FPS)
    │   └── capture_bench.rs # Диагностический инструмент DXGI/D3D11
```

---

## 4. Следующие шаги (Переход к MVP)

1. **Кодирование видео кадра (host-encode):**
   * Передача захваченной `ID3D11Texture2D` в аппаратный энкодер NVENC (H.264 / HEVC) на RTX 2060.
2. **Сетевой цикл:**
   * Отправка сжатых пакетов через UDP/QUIC датаграммы локальному клиенту.
3. **Клиентский рендеринг (client-render):**
   * Приём пакетов, сборка через `FrameReassembler`, аппаратное декодирование и вывод в D3D11 SwapChain.
