// irchad: Windows twin of src/lazy_load_deps_for/nvidia (the Linux implib
// trampolines). Defines the CUDA driver API calls the NVENC encoder makes and
// forwards each to nvcuda.dll, loaded on first use. Nothing links against
// nvcuda.lib, so the binary starts on machines without an Nvidia driver;
// there every call fails with CUDA_ERROR_SHARED_OBJECT_INIT_FAILED and the
// encoder factory reports NVENC as unavailable.
//
// cuda.h maps some names to versioned symbols (cuCtxCreate -> cuCtxCreate_v2),
// so each definition below is really the versioned one, and CU_NAME looks up
// the matching export.

#include <cuda.h>
#include <windows.h>

namespace {

HMODULE NvCuda() {
  static const HMODULE module =
      LoadLibraryExW(L"nvcuda.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
  return module;
}

template <typename F>
F Resolve(const char* name) {
  HMODULE module = NvCuda();
  return module ? reinterpret_cast<F>(GetProcAddress(module, name)) : nullptr;
}

constexpr CUresult kUnavailable = CUDA_ERROR_SHARED_OBJECT_INIT_FAILED;

}  // namespace

#define CU_NAME2(x) #x
#define CU_NAME(x) CU_NAME2(x)

// Defines `name` with `params`, forwarding `args` to nvcuda.dll's export.
#define CU_FORWARD(name, params, args)                              \
  CUresult CUDAAPI name params {                                    \
    static const auto fn = Resolve<decltype(&name)>(CU_NAME(name)); \
    return fn ? fn args : kUnavailable;                             \
  }

extern "C" {

CUresult CUDAAPI cuGetErrorName(CUresult error, const char** pStr) {
  static const auto fn =
      Resolve<decltype(&cuGetErrorName)>(CU_NAME(cuGetErrorName));
  if (fn) {
    return fn(error, pStr);
  }
  // Callers stream *pStr into a log, so never leave it null.
  if (pStr) {
    *pStr = "nvcuda.dll unavailable";
  }
  return kUnavailable;
}

CU_FORWARD(cuInit, (unsigned int Flags), (Flags))
CU_FORWARD(cuDriverGetVersion, (int* driverVersion), (driverVersion))
CU_FORWARD(cuDeviceGet, (CUdevice * device, int ordinal), (device, ordinal))
CU_FORWARD(cuDeviceGetCount, (int* count), (count))
CU_FORWARD(cuDeviceGetName,
           (char* name, int len, CUdevice dev),
           (name, len, dev))
CU_FORWARD(cuDeviceGetAttribute,
           (int* pi, CUdevice_attribute attrib, CUdevice dev),
           (pi, attrib, dev))
#if CUDA_VERSION >= 13000
CU_FORWARD(cuCtxCreate,
           (CUcontext * pctx,
            CUctxCreateParams* ctxCreateParams,
            unsigned int flags,
            CUdevice dev),
           (pctx, ctxCreateParams, flags, dev))
#else
CU_FORWARD(cuCtxCreate,
           (CUcontext * pctx, unsigned int flags, CUdevice dev),
           (pctx, flags, dev))
#endif
CU_FORWARD(cuCtxDestroy, (CUcontext ctx), (ctx))
CU_FORWARD(cuCtxGetCurrent, (CUcontext * pctx), (pctx))
CU_FORWARD(cuCtxSetCurrent, (CUcontext ctx), (ctx))
CU_FORWARD(cuCtxGetDevice, (CUdevice * device), (device))
CU_FORWARD(cuCtxPushCurrent, (CUcontext ctx), (ctx))
CU_FORWARD(cuCtxPopCurrent, (CUcontext * pctx), (pctx))
CU_FORWARD(cuMemAllocPitch,
           (CUdeviceptr * dptr,
            size_t* pPitch,
            size_t WidthInBytes,
            size_t Height,
            unsigned int ElementSizeBytes),
           (dptr, pPitch, WidthInBytes, Height, ElementSizeBytes))
// irchad: cuMemAlloc and the stream calls are NVDEC's (NvDecoder.cpp), not
// the encoder's.
CU_FORWARD(cuMemAlloc, (CUdeviceptr* dptr, size_t bytesize), (dptr, bytesize))
CU_FORWARD(cuMemFree, (CUdeviceptr dptr), (dptr))
CU_FORWARD(cuArrayDestroy, (CUarray hArray), (hArray))
CU_FORWARD(cuMemcpy2D, (const CUDA_MEMCPY2D* pCopy), (pCopy))
CU_FORWARD(cuMemcpy2DUnaligned, (const CUDA_MEMCPY2D* pCopy), (pCopy))
CU_FORWARD(cuMemcpy2DAsync,
           (const CUDA_MEMCPY2D* pCopy, CUstream hStream),
           (pCopy, hStream))
CU_FORWARD(cuStreamCreate, (CUstream* phStream, unsigned int Flags),
           (phStream, Flags))
CU_FORWARD(cuStreamSynchronize, (CUstream hStream), (hStream))
CU_FORWARD(cuStreamDestroy, (CUstream hStream), (hStream))

}  // extern "C"
