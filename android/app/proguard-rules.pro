# JNA (used by the UniFFI-generated bindings) looks classes up reflectively.
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-dontwarn java.awt.**
# Generated UniFFI bindings: callback interfaces and structures are reached through JNA.
-keep class be.alwyapps.watch.core.** { *; }
