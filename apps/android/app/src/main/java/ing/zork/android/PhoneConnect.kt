package ing.zork.android

import androidx.compose.runtime.Composable

@Composable
internal fun PhoneConnectActions(model: ClientViewModel) {
    AccountContent(model.account, model.accountError, model::accountAction)
}
