# WebView 的 JS 桥接方法必须保留；当前 release 没开 minify，这里先留着。
-keepclassmembers class dev.wiretoss.mobile.WebViewActivity$NativeBridge {
    public *;
}
