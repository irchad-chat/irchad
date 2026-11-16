// irchad: Windows loader for nvcuvid.dll (the NVDEC entry points), the twin
// of cuda_loader_win.cpp for the driver API and of src/lazy_load_deps_for/
// nvidia's nvcuvid trampolines on Linux. Nothing links nvcuvid.lib, so the
// binary starts on machines without an Nvidia driver; there every call fails
// with CUDA_ERROR_SHARED_OBJECT_INIT_FAILED and the decoder factory reports
// NVDEC as unavailable.
//
// On x64, cuviddec.h maps cuvidMapVideoFrame to cuvidMapVideoFrame64 (the
// DLL exports the width-matching name), so the 64-bit forms are defined
// here; no call site references the plain names on _WIN64.

#include <cuda.h>
#include <nvcuvid.h>
#include <windows.h>

namespace {

HMODULE NvCuvid() {
  static const HMODULE module =
      LoadLibraryExW(L"nvcuvid.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
  return module;
}

template <typename F>
F Resolve(const char* name) {
  HMODULE module = NvCuvid();
  return module ? reinterpret_cast<F>(GetProcAddress(module, name)) : nullptr;
}

constexpr CUresult kUnavailable = CUDA_ERROR_SHARED_OBJECT_INIT_FAILED;

}  // namespace

// Defines `name` with `params`, forwarding `args` to nvcuvid.dll's export.
#define CUVID_FORWARD(name, params, args)                            \
  CUresult CUDAAPI name params {                                      \
    static const auto fn = Resolve<decltype(&name)>(#name);           \
    return fn ? fn args : kUnavailable;                               \
  }

extern "C" {

CUVID_FORWARD(cuvidGetDecoderCaps, (CUVIDDECODECAPS* pdc), (pdc))
CUVID_FORWARD(cuvidCreateDecoder,
              (CUvideodecoder * phDecoder, CUVIDDECODECREATEINFO* pdci),
              (phDecoder, pdci))
CUVID_FORWARD(cuvidDestroyDecoder, (CUvideodecoder hDecoder), (hDecoder))
CUVID_FORWARD(cuvidDecodePicture,
              (CUvideodecoder hDecoder, CUVIDPICPARAMS* pPicParams),
              (hDecoder, pPicParams))
CUVID_FORWARD(cuvidGetDecodeStatus,
              (CUvideodecoder hDecoder,
               int nPicIdx,
               CUVIDGETDECODESTATUS* pDecodeStatus),
              (hDecoder, nPicIdx, pDecodeStatus))
CUVID_FORWARD(cuvidReconfigureDecoder,
              (CUvideodecoder hDecoder, CUVIDRECONFIGUREDECODERINFO* pReconfig),
              (hDecoder, pReconfig))
CUVID_FORWARD(cuvidMapVideoFrame64,
              (CUvideodecoder hDecoder,
               int nPicIdx,
               unsigned long long* pDevPtr,
               unsigned int* pPitch,
               CUVIDPROCPARAMS* pVPP),
              (hDecoder, nPicIdx, pDevPtr, pPitch, pVPP))
CUVID_FORWARD(cuvidUnmapVideoFrame64,
              (CUvideodecoder hDecoder, unsigned long long DevPtr),
              (hDecoder, DevPtr))
CUVID_FORWARD(cuvidCtxLockCreate, (CUvideoctxlock* pLock, CUcontext ctx),
              (pLock, ctx))
CUVID_FORWARD(cuvidCtxLockDestroy, (CUvideoctxlock lck), (lck))
CUVID_FORWARD(cuvidCreateVideoParser,
              (CUvideoparser * pObj, CUVIDPARSERPARAMS* pParams),
              (pObj, pParams))
CUVID_FORWARD(cuvidDestroyVideoParser, (CUvideoparser obj), (obj))
CUVID_FORWARD(cuvidParseVideoData,
              (CUvideoparser obj, CUVIDSOURCEDATAPACKET* pPacket),
              (obj, pPacket))

}  // extern "C"
