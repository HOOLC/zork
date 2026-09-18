package ing.zork.android

import android.annotation.SuppressLint
import android.app.Activity
import android.app.Dialog
import android.content.Context
import android.content.ContextWrapper
import android.net.Uri
import android.view.ViewGroup
import android.view.KeyEvent
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The dialog stays in the foreground Activity, so opening it does not pause Mesh. */
@SuppressLint("SetJavaScriptEnabled")
internal fun openServiceBrowser(context: Context, sharedUrl: String) {
    var base = context
    while (base is ContextWrapper && base !is Activity) base = base.baseContext
    val activity = base as? Activity ?: return
    val lifecycle = (activity as? LifecycleOwner)?.lifecycle ?: return
    val repo = ClientRepository(activity)
    val viewId = NativeBridge.newId()
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val dialog = Dialog(activity, R.style.Theme_Zork)
    val root = LinearLayout(activity).apply { orientation = LinearLayout.VERTICAL }
    val toolbar = LinearLayout(activity)
    fun dp(value: Int) = (value * activity.resources.displayMetrics.density).toInt()
    val title = TextView(activity).apply {
        text = "正在连接服务…"
        setPadding(dp(12), dp(12), dp(12), dp(12))
        maxLines = 2
    }
    val web = WebView(activity).apply {
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        settings.setSupportMultipleWindows(false)
        android.webkit.CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
    }
    toolbar.addView(Button(activity).apply { text = "关闭"; setOnClickListener { dialog.dismiss() } })
    val back = Button(activity).apply { text = "返回"; isEnabled = false; setOnClickListener { web.goBack() } }
    toolbar.addView(back)
    toolbar.addView(title, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
    toolbar.addView(Button(activity).apply { text = "刷新"; setOnClickListener { web.reload() } })
    root.addView(toolbar)
    root.addView(web, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
    dialog.setContentView(root)
    dialog.setOnKeyListener { _, key, event ->
        if (key == KeyEvent.KEYCODE_BACK && event.action == KeyEvent.ACTION_UP && web.canGoBack()) {
            web.goBack()
            true
        } else false
    }
    // Backgrounding closes this transient view. Reopening creates a fresh listener;
    // it never leaves a local forwarding port alive after the client pauses.
    val observer = LifecycleEventObserver { _, event ->
        if (event == Lifecycle.Event.ON_STOP || event == Lifecycle.Event.ON_DESTROY) dialog.dismiss()
    }
    lifecycle.addObserver(observer)
    dialog.setOnDismissListener {
        lifecycle.removeObserver(observer)
        scope.cancel()
        web.stopLoading()
        root.removeView(web)
        web.destroy()
    }
    dialog.show()
    dialog.window?.setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
    scope.launch {
        try {
            val result = repo.command("open_service", "view_id" to viewId, "url" to sharedUrl)
            val url = result.getString("url")
            val serviceHost = Uri.parse(url).host
            web.webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    val target = request.url
                    return when (target.scheme) {
                        "http" -> target.host != serviceHost
                        "https" -> false
                        else -> true
                    }
                }
                override fun onPageFinished(view: WebView, url: String) {
                    title.text = view.title?.takeIf { it.isNotBlank() } ?: "共享服务"
                    back.isEnabled = view.canGoBack()
                }
            }
            web.loadUrl(url)
            awaitCancellation()
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            title.text = error.message ?: "服务连接失败，请关闭后重试"
        } finally {
            withContext(NonCancellable) {
                runCatching { repo.command("close_service", "view_id" to viewId) }
            }
        }
    }
}
