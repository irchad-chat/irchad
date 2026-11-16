// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).
//
// Windows only, and free of webrtc headers so it can be checked on its own.

#include "amf_runtime.h"

#include <windows.h>

// d3d10_1.h, not d3d10.h: it includes d3d10.h itself, and the SDK errors out
// when d3d10.h comes first and something later pulls in d3d10_1.h.
#include <d3d10_1.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <wrl/client.h>

using Microsoft::WRL::ComPtr;

namespace livekit_ffi {

namespace {

constexpr UINT kAmdVendorId = 0x1002;

ComPtr<ID3D11Device> CreateAmdDevice() {
  ComPtr<IDXGIFactory1> factory;
  if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) {
    return nullptr;
  }
  // Fastest first where the OS can say (Windows 10 1803 and later), so a
  // discrete Radeon wins over the integrated one beside it.
  ComPtr<IDXGIFactory6> factory6;
  factory.As(&factory6);

  for (UINT i = 0;; ++i) {
    ComPtr<IDXGIAdapter1> adapter;
    const HRESULT found =
        factory6 ? factory6->EnumAdapterByGpuPreference(
                       i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
                       IID_PPV_ARGS(&adapter))
                 : factory->EnumAdapters1(i, &adapter);
    if (FAILED(found)) {
      return nullptr;
    }

    DXGI_ADAPTER_DESC1 desc = {};
    if (FAILED(adapter->GetDesc1(&desc)) || desc.VendorId != kAmdVendorId ||
        (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE)) {
      continue;
    }

    const D3D_FEATURE_LEVEL levels[] = {D3D_FEATURE_LEVEL_11_1,
                                        D3D_FEATURE_LEVEL_11_0};
    ComPtr<ID3D11Device> device;
    if (FAILED(D3D11CreateDevice(adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN,
                                 nullptr, 0, levels, ARRAYSIZE(levels),
                                 D3D11_SDK_VERSION, &device, nullptr,
                                 nullptr))) {
      continue;
    }

    // AMF uses the device from its own threads.
    ComPtr<ID3D10Multithread> multithread;
    if (SUCCEEDED(device.As(&multithread))) {
      multithread->SetMultithreadProtected(TRUE);
    }
    return device;
  }
}

amf::AMFFactory* LoadAmfFactory() {
  // System32 only: the driver puts it there, and no other folder gets to
  // supply a DLL we would run.
  HMODULE dll =
      LoadLibraryExW(AMF_DLL_NAME, nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
  if (!dll) {
    return nullptr;
  }
  auto init = reinterpret_cast<AMFInit_Fn>(
      reinterpret_cast<void*>(GetProcAddress(dll, AMF_INIT_FUNCTION_NAME)));
  amf::AMFFactory* factory = nullptr;
  if (!init || init(AMF_FULL_VERSION, &factory) != AMF_OK || !factory) {
    FreeLibrary(dll);
    return nullptr;
  }
  return factory;
}

}  // namespace

amf::AMFFactory* AmfFactory() {
  static amf::AMFFactory* const factory = LoadAmfFactory();
  return factory;
}

amf::AMFContextPtr CreateAmfContext() {
  amf::AMFFactory* factory = AmfFactory();
  if (!factory) {
    return nullptr;
  }
  ComPtr<ID3D11Device> device = CreateAmdDevice();
  if (!device) {
    return nullptr;
  }
  amf::AMFContextPtr context;
  if (factory->CreateContext(&context) != AMF_OK || !context) {
    return nullptr;
  }
  // The context takes its own reference on the device.
  if (context->InitDX11(device.Get()) != AMF_OK) {
    return nullptr;
  }
  return context;
}

}  // namespace livekit_ffi
