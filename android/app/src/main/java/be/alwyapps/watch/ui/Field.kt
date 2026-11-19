// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.input.VisualTransformation

/**
 * Text field that behaves on TV: focusing it (moving down the screen) does not open the
 * keyboard, which would trap the D-pad and type its highlighted key on the next OK. The field
 * stays read-only until OK is released, then the keyboard opens. Phones: a plain field.
 */
@Composable
fun Field(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    label: (@Composable () -> Unit)? = null,
    placeholder: (@Composable () -> Unit)? = null,
    singleLine: Boolean = false,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
    visualTransformation: VisualTransformation = VisualTransformation.None,
) {
    val isTv = LocalDimens.current.isTv
    var editing by remember { mutableStateOf(!isTv) }
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(editing) { if (isTv && editing) keyboard?.show() }
    OutlinedTextField(
        value,
        onValueChange,
        modifier
            .onFocusChanged { if (isTv && !it.isFocused) editing = false }
            .onPreviewKeyEvent {
                val ok = it.key == Key.DirectionCenter || it.key == Key.Enter || it.key == Key.NumPadEnter
                if (!isTv || editing || !ok) return@onPreviewKeyEvent false
                if (it.type == KeyEventType.KeyUp) editing = true
                true
            },
        readOnly = !editing,
        label = label,
        placeholder = placeholder,
        singleLine = singleLine,
        keyboardOptions = keyboardOptions,
        keyboardActions = keyboardActions,
        visualTransformation = visualTransformation,
    )
}
