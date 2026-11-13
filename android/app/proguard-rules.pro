# JNA and the UniFFI bindings (chad_ffi.kt) work by reflection: Structure
# fields, Callback interfaces and Native.register look members up by name.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class party.gentoo.irchad.core.** { *; }
-dontwarn java.awt.**
