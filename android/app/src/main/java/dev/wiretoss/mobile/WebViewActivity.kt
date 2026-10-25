package dev.wiretoss.mobile

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.DownloadManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.webkit.CookieManager
import android.webkit.JavascriptInterface
import android.webkit.PermissionRequest
import android.webkit.URLUtil
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import dev.wiretoss.mobile.ui.WtAccent
import dev.wiretoss.mobile.ui.WtBackground
import dev.wiretoss.mobile.ui.WtBackgroundDeep
import dev.wiretoss.mobile.ui.WtDanger
import dev.wiretoss.mobile.ui.WtLogo
import dev.wiretoss.mobile.ui.WtPrimaryButton
import dev.wiretoss.mobile.ui.WtSecondaryButton
import dev.wiretoss.mobile.ui.WtSurface
import dev.wiretoss.mobile.ui.WtText
import dev.wiretoss.mobile.ui.WtTextDim
import dev.wiretoss.mobile.ui.WiretossTheme

class WebViewActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = false
            isAppearanceLightNavigationBars = false
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q &&
            checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE), 1002)
        }

        val url = intent.getStringExtra(EXTRA_URL).orEmpty()
        setContent {
            WiretossTheme {
                WebScreen(url = url, onExit = { finish() })
            }
        }
    }

    companion object {
        const val EXTRA_URL = "url"
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun WebScreen(url: String, onExit: () -> Unit) {
    val context = LocalContext.current
    var error by remember { mutableStateOf(if (url.isBlank()) "服务器地址无效。" else null) }
    var reloadKey by remember { mutableIntStateOf(0) }
    var fileCallback by remember { mutableStateOf<ValueCallback<Array<Uri>>?>(null) }

    val fileLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val callback = fileCallback
        fileCallback = null
        if (callback == null) return@rememberLauncherForActivityResult

        if (result.resultCode != Activity.RESULT_OK) {
            callback.onReceiveValue(null)
            return@rememberLauncherForActivityResult
        }

        val data = result.data
        val uris: Array<Uri>? = when {
            data?.clipData != null -> {
                val clip = data.clipData!!
                Array(clip.itemCount) { index -> clip.getItemAt(index).uri }
            }
            data?.data != null -> arrayOf(data.data!!)
            else -> null
        }
        callback.onReceiveValue(uris)
    }

    val webView = remember {
        WebView(context).apply {
            setBackgroundColor(0xFF0B1020.toInt())
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.useWideViewPort = true
            settings.loadWithOverviewMode = true
            settings.allowFileAccess = false
            settings.allowContentAccess = true
            settings.mixedContentMode = WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE
            settings.userAgentString = settings.userAgentString + " WiretossAndroid/0.1.0"

            CookieManager.getInstance().setAcceptCookie(true)
            CookieManager.getInstance().setAcceptThirdPartyCookies(this, true)

            addJavascriptInterface(
                object {
                    @JavascriptInterface
                    fun showServers() {
                        (context as? Activity)?.runOnUiThread { onExit() }
                    }
                },
                "WiretossNative",
            )

            webViewClient = object : WebViewClient() {
                override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                    error = null
                }

                override fun onPageFinished(view: WebView, url: String?) {
                    view.evaluateJavascript(BRIDGE_JS, null)
                }

                override fun onReceivedError(
                    view: WebView,
                    request: WebResourceRequest,
                    errorDesc: WebResourceError,
                ) {
                    if (request.isForMainFrame) {
                        error = friendlyWebError(errorDesc)
                    }
                }

                override fun shouldOverrideUrlLoading(
                    view: WebView,
                    request: WebResourceRequest,
                ): Boolean {
                    val scheme = request.url.scheme?.lowercase()
                    return scheme != "http" && scheme != "https"
                }
            }

            webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(
                    view: WebView?,
                    callback: ValueCallback<Array<Uri>>?,
                    params: FileChooserParams?,
                ): Boolean {
                    if (callback == null) return false
                    fileCallback?.onReceiveValue(null)
                    fileCallback = callback

                    val intent = params?.createIntent() ?: Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)

                    return try {
                        fileLauncher.launch(Intent.createChooser(intent, "选择要上传的文件"))
                        true
                    } catch (_: Exception) {
                        fileCallback = null
                        callback.onReceiveValue(null)
                        false
                    }
                }

                override fun onPermissionRequest(request: PermissionRequest) {
                    request.deny()
                }
            }

            setDownloadListener { downloadUrl, _, contentDisposition, mimeType, _ ->
                enqueueDownload(context, downloadUrl, contentDisposition, mimeType)
            }
        }
    }

    androidx.compose.runtime.LaunchedEffect(url, reloadKey) {
        if (url.isNotBlank()) webView.loadUrl(url)
    }

    DisposableEffect(webView) {
        onDispose {
            webView.stopLoading()
            webView.destroy()
        }
    }

    BackHandler { onExit() }

    Box(
        Modifier
            .fillMaxSize()
            .background(WtBackgroundDeep)
            .windowInsetsPadding(WindowInsets.safeDrawing)
    ) {
        AndroidView(
            factory = { webView },
            modifier = Modifier.fillMaxSize(),
        )
        val message = error
        if (message != null) {
            ErrorOverlay(
                message = message,
                onRetry = {
                    error = null
                    webView.reload()
                },
                onExit = onExit,
            )
        }
    }
}

@Composable
private fun ErrorOverlay(message: String, onRetry: () -> Unit, onExit: () -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .background(WtBackgroundDeep.copy(alpha = 0.97f)),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .padding(24.dp)
                .widthIn(max = 420.dp)
                .fillMaxWidth()
                .clip(RoundedCornerShape(22.dp))
                .background(WtSurface)
                .border(1.dp, Color.White.copy(alpha = 0.08f), RoundedCornerShape(22.dp))
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            WtLogo(56.dp)
            Text(
                "无法连接服务器",
                color = WtText,
                fontSize = 20.sp,
                fontWeight = FontWeight.Bold,
            )
            Text(
                message,
                color = WtTextDim,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
            WtPrimaryButton(onClick = onRetry) {
                Icon(Icons.Rounded.Refresh, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("重试", fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            }
            WtSecondaryButton(onClick = onExit) {
                Text("返回服务器列表", fontSize = 14.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}

private fun friendlyWebError(error: WebResourceError): String = when (error.errorCode) {
    WebViewClient.ERROR_HOST_LOOKUP -> "找不到服务器地址。确认 IP 或主机名是否正确。"
    WebViewClient.ERROR_CONNECT -> "连不上服务器。确认电脑上的 Wiretoss 正在运行。"
    WebViewClient.ERROR_TIMEOUT -> "连接超时。检查 IP 地址、端口和防火墙设置。"
    WebViewClient.ERROR_UNKNOWN -> "页面加载失败，请重试。"
    else -> error.description?.toString() ?: "页面加载失败，请重试。"
}

private fun enqueueDownload(
    context: Context,
    url: String,
    contentDisposition: String?,
    mimeType: String?,
) {
    val cookie = CookieManager.getInstance().getCookie(url).orEmpty()
    var fileName = URLUtil.guessFileName(url, contentDisposition, mimeType)
        .replace('/', '_')
        .replace('\\', '_')
    if (fileName.isBlank()) fileName = "wiretoss-download"

    val request = DownloadManager.Request(Uri.parse(url))
        .setTitle(fileName)
        .setDescription("Wiretoss")
        .setAllowedOverMetered(true)
        .setAllowedOverRoaming(false)
        .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
        .setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS, fileName)
    mimeType?.let { request.setMimeType(it) }
    if (cookie.isNotBlank()) request.addRequestHeader("Cookie", cookie)

    try {
        val manager = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
        manager.enqueue(request)
        Toast.makeText(context, "已开始下载：$fileName", Toast.LENGTH_SHORT).show()
    } catch (_: Exception) {
        Toast.makeText(context, "无法开始下载，请检查存储权限。", Toast.LENGTH_LONG).show()
    }
}

private const val BRIDGE_JS = """
(function () {
  if (window.__wiretossNativeInstalled) return;
  window.__wiretossNativeInstalled = true;
  document.documentElement.classList.add('in-wiretoss-app');

  var style = document.createElement('style');
  style.textContent = '.in-wiretoss-app #upload-folder{display:none!important}' +
    '.wiretoss-native-servers-float{position:fixed;top:calc(env(safe-area-inset-top) + 12px);right:12px;z-index:90;background:rgba(21,27,46,.84);border:1px solid rgba(255,255,255,.12);backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px)}';
  document.head.appendChild(style);

  function makeButton() {
    var button = document.createElement('button');
    button.type = 'button';
    button.className = 'icon-btn wiretoss-native-servers-inline';
    button.title = '切换服务器';
    button.setAttribute('aria-label', '切换服务器');
    button.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h16"/></svg>';
    button.addEventListener('click', function () {
      if (window.WiretossNative && window.WiretossNative.showServers) {
        window.WiretossNative.showServers();
      }
    });
    return button;
  }

  function place() {
    var app = document.getElementById('app-view');
    var topbar = app && !app.hidden ? document.querySelector('.topbar-right') : null;
    var floating = document.getElementById('wiretoss-native-servers');

    if (topbar) {
      if (floating) floating.remove();
      if (!topbar.querySelector('.wiretoss-native-servers-inline')) {
        topbar.insertBefore(makeButton(), topbar.firstChild);
      }
      return;
    }

    var login = document.getElementById('login-view');
    if (login && !login.hidden && !floating) {
      var button = makeButton();
      button.id = 'wiretoss-native-servers';
      button.classList.remove('wiretoss-native-servers-inline');
      button.classList.add('wiretoss-native-servers-float');
      document.body.appendChild(button);
    }
  }

  place();
  new MutationObserver(place).observe(document.body, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['hidden', 'class']
  });
})();
"""
