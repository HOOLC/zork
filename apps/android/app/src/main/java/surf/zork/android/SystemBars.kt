package surf.zork.android

import android.graphics.Color
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind

internal fun ComponentActivity.configureZorkSystemBars() {
    enableEdgeToEdge(
        statusBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT),
        navigationBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT),
    )
    window.isNavigationBarContrastEnforced = false
}

/** Paint behind both transparent system bars; apply safe insets to content inside. */
@Composable
internal fun ZorkPageBackground(lightPage: Boolean, content: @Composable BoxScope.() -> Unit) {
    val background = animateColorAsState(
        if (lightPage) ZorkColors.Canvas else ZorkColors.Paper,
        animationSpec = PageSlideMotion.spec(lightPage), label = "system-bar-background",
    )
    Box(Modifier.fillMaxSize().drawBehind { drawRect(background.value) }, content = content)
}
