package dev.wiretoss.mobile.ui

import android.app.Activity
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowForward
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.Public
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Wifi
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.wiretoss.mobile.DiscoveredServer
import dev.wiretoss.mobile.NetworkProbe
import dev.wiretoss.mobile.Server
import dev.wiretoss.mobile.ServerAddress
import dev.wiretoss.mobile.ServerDiscovery
import dev.wiretoss.mobile.ServerStore
import dev.wiretoss.mobile.WebViewActivity
import kotlinx.coroutines.launch

@Composable
fun SetupScreen() {
    val context = LocalContext.current
    val activity = context as? Activity
    val store = remember { ServerStore(context.applicationContext) }
    val scope = rememberCoroutineScope()
    val focusManager = LocalFocusManager.current
    val discovery = remember { ServerDiscovery(context.applicationContext) }

    var saved by remember { mutableStateOf(store.load()) }
    var discovered by remember { mutableStateOf<List<DiscoveredServer>>(emptyList()) }
    var searching by remember { mutableStateOf(false) }
    var discoveryError by remember { mutableStateOf<String?>(null) }
    var address by rememberSaveable { mutableStateOf("") }
    var connecting by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun connect(raw: String) {
        if (connecting) return
        val normalized = ServerAddress.normalize(raw)
        if (normalized == null) {
            error = "请输入有效的地址，例如 192.168.1.5:8080"
            return
        }

        focusManager.clearFocus()
        connecting = true
        error = null

        scope.launch {
            val result = NetworkProbe.check(normalized)
            connecting = false
            result
                .onSuccess {
                    val server = Server(
                        id = normalized,
                        name = ServerAddress.displayName(normalized),
                        url = normalized,
                        lastUsed = System.currentTimeMillis(),
                    )
                    saved = store.upsert(server)
                    val intent = Intent(context, WebViewActivity::class.java)
                        .putExtra(WebViewActivity.EXTRA_URL, normalized)
                    if (activity != null) {
                        activity.startActivity(intent)
                    } else {
                        context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }
                }
                .onFailure {
                    error = NetworkProbe.friendlyMessage(it)
                }
        }
    }

    val scanOptions = remember {
        ScanOptions()
            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt("把电脑上的二维码放进取景框")
            .setBeepEnabled(false)
            .setOrientationLocked(false)
    }
    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        val content = result.contents ?: return@rememberLauncherForActivityResult
        val normalized = ServerAddress.normalize(content)
        if (normalized == null) {
            error = "二维码里不是一个有效的 Wiretoss 地址。"
        } else {
            address = normalized
            connect(normalized)
        }
    }

    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        saved = store.load()
    }

    DisposableEffect(discovery) {
        discovery.onUpdate = { discovered = it }
        discovery.onSearching = { searching = it }
        discovery.onError = { discoveryError = it }
        discovery.start()
        onDispose { discovery.stop() }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        WtBackground()

        val expanded = maxWidth >= 720.dp && maxHeight >= 600.dp
        if (expanded) {
            Row(
                Modifier
                    .fillMaxSize()
                    .windowInsetsPadding(WindowInsets.safeDrawing)
                    .imePadding()
                    .padding(horizontal = 32.dp, vertical = 32.dp),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    Modifier
                        .widthIn(max = 1200.dp)
                        .fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(32.dp),
                    verticalAlignment = Alignment.Top,
                ) {
                    TabletHero(Modifier.weight(0.85f))
                    Column(
                        Modifier
                            .weight(1.15f)
                            .heightIn(max = 620.dp)
                            .verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        SetupCards(
                            address = address,
                            onAddressChange = { address = it },
                            connecting = connecting,
                            error = error,
                            onConnect = { connect(address) },
                            onScan = { scanLauncher.launch(scanOptions) },
                            discovered = discovered,
                            searching = searching,
                            discoveryError = discoveryError,
                            onRefreshDiscovery = {
                                discoveryError = null
                                discovery.stop()
                                discovery.start()
                            },
                            onSelectDiscovered = {
                                address = it.url
                                connect(it.url)
                            },
                            saved = saved,
                            onSelectSaved = {
                                address = it.url
                                connect(it.url)
                            },
                            onDeleteSaved = { saved = store.delete(it.id) },
                        )
                    }
                }
            }
        } else {
            Column(
                Modifier
                    .fillMaxSize()
                    .windowInsetsPadding(WindowInsets.safeDrawing)
                    .imePadding()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 26.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Column(
                    Modifier
                        .widthIn(max = 560.dp)
                        .fillMaxWidth(),
                    verticalArrangement = Arrangement.spacedBy(18.dp),
                ) {
                    PhoneHeader()
                    SetupCards(
                        address = address,
                        onAddressChange = { address = it },
                        connecting = connecting,
                        error = error,
                        onConnect = { connect(address) },
                        onScan = { scanLauncher.launch(scanOptions) },
                        discovered = discovered,
                        searching = searching,
                        discoveryError = discoveryError,
                        onRefreshDiscovery = {
                            discoveryError = null
                            discovery.stop()
                            discovery.start()
                        },
                        onSelectDiscovered = {
                            address = it.url
                            connect(it.url)
                        },
                        saved = saved,
                        onSelectSaved = {
                            address = it.url
                            connect(it.url)
                        },
                        onDeleteSaved = { saved = store.delete(it.id) },
                    )
                }
            }
        }
    }
}

@Composable
private fun PhoneHeader() {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(top = 8.dp, bottom = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        WtLogo(68.dp)
        Spacer(Modifier.height(14.dp))
        Text(
            "Wiretoss",
            color = WtText,
            fontSize = 32.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            "连接电脑上的局域网文件快传",
            color = WtTextDim,
            fontSize = 13.5.sp,
        )
    }
}

@Composable
private fun TabletHero(modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        WtLogo(92.dp)
        Text(
            "Wiretoss",
            color = WtText,
            fontSize = 40.sp,
            fontWeight = FontWeight.Bold,
        )
        Text(
            "电脑上的文件，平板随手拿。",
            color = WtTextDim,
            fontSize = 17.sp,
        )
        Spacer(Modifier.height(4.dp))
        HeroBullet(Icons.Rounded.QrCodeScanner, "扫描电脑上的二维码即可连接")
        HeroBullet(Icons.Rounded.Wifi, "自动发现同一局域网里的设备")
        HeroBullet(Icons.Rounded.Download, "下载交给系统，支持分享和保存")
        Spacer(Modifier.height(10.dp))
        Text(
            "数据只在局域网内传输 · 不经过云端",
            color = WtTextFaint,
            fontSize = 11.sp,
        )
    }
}

@Composable
private fun HeroBullet(icon: androidx.compose.ui.graphics.vector.ImageVector, text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(
            Modifier
                .size(38.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(WtAccent.copy(alpha = 0.12f)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = WtAccent, modifier = Modifier.size(19.dp))
        }
        Spacer(Modifier.width(13.dp))
        Text(text, color = WtTextDim, fontSize = 13.5.sp)
    }
}

@Composable
private fun SetupCards(
    address: String,
    onAddressChange: (String) -> Unit,
    connecting: Boolean,
    error: String?,
    onConnect: () -> Unit,
    onScan: () -> Unit,
    discovered: List<DiscoveredServer>,
    searching: Boolean,
    discoveryError: String?,
    onRefreshDiscovery: () -> Unit,
    onSelectDiscovered: (DiscoveredServer) -> Unit,
    saved: List<Server>,
    onSelectSaved: (Server) -> Unit,
    onDeleteSaved: (Server) -> Unit,
) {
    ConnectCard(
        address = address,
        onAddressChange = onAddressChange,
        connecting = connecting,
        onConnect = onConnect,
        onScan = onScan,
    )

    if (error != null) {
        WtErrorBanner(error)
    }

    DiscoveryCard(
        discovered = discovered,
        searching = searching,
        error = discoveryError,
        onRefresh = onRefreshDiscovery,
        onSelect = onSelectDiscovered,
    )

    if (saved.isNotEmpty()) {
        SavedCard(
            saved = saved,
            onSelect = onSelectSaved,
            onDelete = onDeleteSaved,
        )
    }

    Text(
        "数据只在局域网内传输 · 不经过云端",
        color = WtTextFaint,
        fontSize = 11.sp,
        modifier = Modifier.padding(top = 4.dp),
    )
}

@Composable
private fun ConnectCard(
    address: String,
    onAddressChange: (String) -> Unit,
    connecting: Boolean,
    onConnect: () -> Unit,
    onScan: () -> Unit,
) {
    val focusManager = LocalFocusManager.current

    WtCard {
        WtSectionTitle("连接到电脑", Icons.Rounded.Computer)
        Spacer(Modifier.height(14.dp))
        OutlinedTextField(
            value = address,
            onValueChange = onAddressChange,
            modifier = Modifier.fillMaxWidth(),
            placeholder = {
                Text(
                    "192.168.1.5:8080",
                    color = WtTextFaint,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 15.sp,
                )
            },
            leadingIcon = {
                Icon(
                    Icons.Rounded.Public,
                    contentDescription = null,
                    tint = WtTextFaint,
                    modifier = Modifier.size(18.dp),
                )
            },
            singleLine = true,
            shape = RoundedCornerShape(13.dp),
            textStyle = TextStyle(
                color = WtText,
                fontFamily = FontFamily.Monospace,
                fontSize = 15.sp,
            ),
            keyboardOptions = KeyboardOptions(
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Go,
            ),
            keyboardActions = KeyboardActions(
                onGo = {
                    focusManager.clearFocus()
                    onConnect()
                }
            ),
            colors = OutlinedTextFieldDefaults.colors(
                focusedBorderColor = WtAccent.copy(alpha = 0.65f),
                unfocusedBorderColor = Color.White.copy(alpha = 0.09f),
                focusedContainerColor = Color.Black.copy(alpha = 0.32f),
                unfocusedContainerColor = Color.Black.copy(alpha = 0.32f),
                cursorColor = WtAccent,
                focusedTextColor = WtText,
                unfocusedTextColor = WtText,
                focusedLeadingIconColor = WtTextFaint,
                unfocusedLeadingIconColor = WtTextFaint,
            ),
        )

        Spacer(Modifier.height(14.dp))
        WtPrimaryButton(
            onClick = {
                focusManager.clearFocus()
                onConnect()
            },
            enabled = !connecting,
        ) {
            if (connecting) {
                CircularProgressIndicator(
                    Modifier.size(18.dp),
                    color = Color.White,
                    strokeWidth = 2.dp,
                )
            } else {
                Icon(
                    Icons.Rounded.ArrowForward,
                    contentDescription = null,
                    modifier = Modifier.size(18.dp),
                )
            }
            Spacer(Modifier.width(8.dp))
            Text(
                if (connecting) "正在连接…" else "连接",
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }

        Spacer(Modifier.height(10.dp))
        WtSecondaryButton(onClick = onScan) {
            Icon(
                Icons.Rounded.QrCodeScanner,
                contentDescription = null,
                modifier = Modifier.size(18.dp),
            )
            Spacer(Modifier.width(8.dp))
            Text(
                "扫描电脑上的二维码",
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
            )
        }

        Spacer(Modifier.height(6.dp))
        Text(
            "手机或平板和电脑需要在同一个 Wi-Fi 或局域网。密码在电脑端 Wiretoss 启动时设置。",
            color = WtTextFaint,
            fontSize = 11.5.sp,
            lineHeight = 17.sp,
        )
    }
}

@Composable
private fun DiscoveryCard(
    discovered: List<DiscoveredServer>,
    searching: Boolean,
    error: String?,
    onRefresh: () -> Unit,
    onSelect: (DiscoveredServer) -> Unit,
) {
    WtCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            WtSectionTitle("自动发现", Icons.Rounded.Wifi)
            Spacer(Modifier.weight(1f))
            if (searching) {
                CircularProgressIndicator(
                    Modifier.size(16.dp),
                    color = WtAccent,
                    strokeWidth = 2.dp,
                )
            }
            Spacer(Modifier.width(8.dp))
            IconButton(
                onClick = onRefresh,
                modifier = Modifier.size(32.dp),
            ) {
                Icon(
                    Icons.Rounded.Refresh,
                    contentDescription = "重新搜索",
                    tint = WtTextDim,
                    modifier = Modifier.size(18.dp),
                )
            }
        }

        Spacer(Modifier.height(10.dp))
        if (error != null) {
            Text(error, color = WtDanger, fontSize = 12.sp)
            Spacer(Modifier.height(8.dp))
        }

        if (discovered.isEmpty()) {
            Text(
                if (searching) {
                    "正在搜索同一局域网里的 Wiretoss…"
                } else {
                    "没有找到设备。确认电脑端的服务已经启动。"
                },
                color = WtTextDim,
                fontSize = 12.5.sp,
            )
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                discovered.forEach { server ->
                    WtServerRow(
                        name = server.name,
                        url = server.url,
                        onClick = { onSelect(server) },
                    )
                }
            }
        }
    }
}

@Composable
private fun SavedCard(
    saved: List<Server>,
    onSelect: (Server) -> Unit,
    onDelete: (Server) -> Unit,
) {
    WtCard {
        WtSectionTitle("常用服务器", Icons.Rounded.History)
        Spacer(Modifier.height(10.dp))
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            saved.forEach { server ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    WtServerRow(
                        name = server.name,
                        url = server.url,
                        onClick = { onSelect(server) },
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(4.dp))
                    IconButton(onClick = { onDelete(server) }) {
                        Icon(
                            Icons.Rounded.DeleteOutline,
                            contentDescription = "删除 ${server.name}",
                            tint = WtTextFaint,
                            modifier = Modifier.size(18.dp),
                        )
                    }
                }
            }
        }
    }
}
