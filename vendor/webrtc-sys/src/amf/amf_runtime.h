// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).

#ifndef AMF_RUNTIME_H_
#define AMF_RUNTIME_H_

#include "core/Context.h"
#include "core/Factory.h"

namespace livekit_ffi {

// The AMF runtime, which the AMD driver installs (amfrt64.dll). Loaded on
// first use and kept for the life of the process. Null when there is no AMD
// driver, so the binary still runs without one.
amf::AMFFactory* AmfFactory();

// A new AMF context on the fastest AMD adapter, or null. Passing no device to
// AMF would take the default adapter, which on a hybrid machine may be
// another vendor's.
amf::AMFContextPtr CreateAmfContext();

}  // namespace livekit_ffi

#endif  // AMF_RUNTIME_H_
