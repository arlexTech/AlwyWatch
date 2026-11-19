// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch

import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import be.alwyapps.watch.core.greet

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val isTv = packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
        val greeting = greet(if (isTv) "Android TV" else "Android")
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Box(
                    modifier = Modifier.fillMaxSize().background(Color(0xFF101114)),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(greeting, color = Color(0xFFE8E8EA), style = MaterialTheme.typography.headlineSmall)
                }
            }
        }
    }
}
