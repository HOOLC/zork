package ing.zork.android

import android.graphics.Rect
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import org.json.JSONObject

/** Visual test harness only. It renders production composables and never connects
 * a client, edits the normal database, or appears in the launcher. */
internal object NewChatFixtureBridge {
    init { System.loadLibrary("zork_android") }
    external fun render(request: String): String
}

class Nav7PreviewActivity : ComponentActivity() {
    var newChatSnapshot: JSONObject? = null
    var contentBounds = Rect()
    var lastAction = ""
    var lastBody: JSONObject? = null
    var lastProfile: JSONObject? = null
    var previewTheme by mutableStateOf("system")
    var failNextRequest = false
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); configureZorkSystemBars()
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val route = intent.getStringExtra("screen") ?: "navigation"
        val width = intent.getIntExtra("width",390)
        setContent {
            CompositionLocalProvider(LocalDensity provides if(width>0) Density(1f, 1f) else LocalDensity.current) {
                ZorkTheme {
                    ZorkPageBackground(lightPage = route != "navigation") {
                        Box((if(width>0) Modifier.requiredSize(width.dp, 844.dp) else Modifier.fillMaxSize().safeDrawingPadding()).onGloballyPositioned {
                            val b = it.boundsInWindow(); contentBounds = Rect(b.left.toInt(), b.top.toInt(), b.right.toInt(), b.bottom.toInt())
                        }) {
                            if (route == "new-chat") {
                                val actions = remember { org.json.JSONArray() }
                                fun project() = JSONObject(NewChatFixtureBridge.render(JSONObject().put("scenario", "draft").put("actions", actions).toString()))
                                var snapshot by remember { mutableStateOf(project()) }
                                newChatSnapshot = snapshot
                                NewChatPage(NewChatUi(fixturePeers()[0], snapshot), {}, { action, value ->
                                    lastAction=action
                                    val intent=JSONObject().put("action",action)
                                    if(value != null) intent.put(if(action=="edit" || action=="submit") "text" else "value",value)
                                    actions.put(intent);lastBody=intent;snapshot=project()
                                }, {})
                            }
                            else if (route in listOf("home","appearance","device","models","profile","connections","services","notifications")) {
                                var settings by remember { mutableStateOf(settingsFixturePage(fixtureSettings(), route)) }
                                val resourceTrail = remember { mutableListOf<ResourceSelection>() }
                                MobileSettings(settings,fixturePeers(),SettingsActions(back={
                                    if(resourceTrail.isNotEmpty()) {
                                        val selected=resourceTrail.removeAt(resourceTrail.lastIndex)
                                        settings=settings.copy(resource=selected,resourceData=settingsResourceFixture(selected))
                                    } else settings=settings.copy(page=if(settings.page=="profile")"models" else "home",resource=null,resourceData=null)
                                }, device={settings=settings.copy(page="device",device=it,resource=null,resourceData=null)},
                                    theme=previewTheme, saveTheme={previewTheme=it},
                                    clearData={lastAction="clear-data"},
                                    notifications=JSONObject("{\"enabled\":true,\"preview\":false,\"sound\":true,\"background\":false,\"muted\":[]}"),
                                    resource={selected->settings.resource?.let{resourceTrail+=it};settings=settings.copy(resource=selected,resourceData=settingsResourceFixture(selected))},
                                    page={settings=settingsFixturePage(settings,it)},profile={settings=settings.copy(page="profile",profile=it)},perform={action,body->
                                    if(failNextRequest){failNextRequest=false;error("fixture request failed")}
                                    lastAction=action;lastBody=JSONObject(body.toString())
                                    fun updateProfile(change:(JSONObject)->Unit):JSONObject {
                                        val profile=JSONObject(settings.profile!!.toString());change(profile);lastProfile=JSONObject(profile.toString())
                                        settings=settings.copy(profile=profile,profiles=settings.profiles.map{if(it.text("profile_id")==profile.text("profile_id"))profile else it})
                                        return profile
                                    }
                                    when {
                                    action=="rename_device" -> JSONObject().put("name",body!!.getString("name"))
                                    action=="rename_profile" -> updateProfile { it.put("name",body!!.getString("name")) }
                                    action=="enable_model" -> updateProfile { profile ->
                                        profile.optJSONArray("models").objects().find{it.text("id")==body.getString("model")}!!.put("enabled",body!!.getBoolean("enabled"))
                                    }
                                    action=="discover_models" -> JSONObject().put("profile",settings.profile).put("added",0).put("configured",0)
                                    action=="save_model" -> updateProfile { profile ->
                                        val result=JSONObject(NativeBridge.previewModels(body.getJSONObject("input").toString(),profile.getJSONArray("models").toString()))
                                        check(result.optBoolean("ok")){result.text("error")}
                                        profile.put("models",result.getJSONObject("data").getJSONArray("models"))
                                    }
                                    action=="save_agent" -> JSONObject()
                                    else -> JSONObject()
                                }}))
                            }
                            else Workbench(fixture(route), WorkbenchActions(resend = { lastAction = "resend:$it" }, deleteFailed = { lastAction = "delete:$it" }, newChat = { lastAction = "new-chat:${it.id}" }, session = { lastAction = "session:${it.optString("_peer")}/${it.text("chat_id")}" }, device = { lastAction = "device:${it.id}" }))
                        }
                    }
                }
            }
        }
    }
}
private fun obj(vararg pairs: Pair<String, Any?>) = JSONObject().apply { pairs.forEach { put(it.first, it.second ?: JSONObject.NULL) } }
private fun leader(id: String, name: String, avatar: String) = obj("id" to id,"name" to name,"avatar" to avatar,"can_open" to true)
private fun task(id: String, title: String, unread: Int = 0) = obj("chat_id" to id,"title" to title,"unread" to (unread > 0),"in_preview" to true)
private fun fixturePeers() = listOf(Peer("mini1","mini1",""), Peer("mini2","mini2",""))
private fun fixture(route: String): WorkbenchState {
    val peers = fixturePeers()
    val product = leader("product","产品 Leader","fox"); val engineering = leader("engineering","工程 Leader","dog"); val research = leader("research","研究 Leader","owl")
    val tasks = mapOf("product" to listOf(task("guide","品牌规范整理",1), task("brand","品牌资源接入")), "engineering" to listOf(task("offline","离线恢复怎么处理",2)))
    val members = listOf(obj("name" to "产品 Leader","avatar" to "fox"),obj("name" to "设计 Worker","avatar" to "cat"),obj("name" to "插画 Worker","avatar" to "panda"))
    if (route == "literal-user") return WorkbenchState(peers=peers,activePeer=peers[0],connected=true,
        conversation=Conversation("literal-user","原文消息验证",canSend=true),
        messages=listOf(ChatMessage("literal-user","你",literalUserFixture,true),
            ChatMessage("literal-assistant","助手","**助手仍用 Markdown**",false)))
    if (route == "delivery") return WorkbenchState(peers=peers,activePeer=peers[0],connected=true,
        conversation=Conversation("delivery","消息发送状态",canSend=true),
        pending=listOf(ChatMessage("failed","你","这条消息发送失败。",true,pending=true,attempted=true,deliveryStatus="failed"),
            ChatMessage("slow","你","超过一秒还没有回执，显示发送中。",true,pending=true,attempted=true,deliveryStatus="sending")))
    if (route == "delivery-error") return WorkbenchState(peers=peers,activePeer=peers[0],connected=true,
        conversation=Conversation("delivery-error","版本不兼容",canSend=true),
        pending=listOf(ChatMessage("incompatible","你","原消息正文",true,pending=true,attempted=true,deliveryStatus="failed",
            deliveryError="目标设备版本过旧，不支持当前客户端发送消息，请先更新目标设备。")))
    return WorkbenchState(peers=peers, activePeer=peers[0], sessions=tasks.values.flatten(), connected=true,
        deviceTrees=mapOf("mini1" to DeviceTree(emptyList(),tasks.values.flatten(),emptyMap(),true), "mini2" to DeviceTree(emptyList(),listOf(task("mobile","移动端交互调研")),emptyMap(),true)),
        conversation=if(route=="navigation") null else Conversation("brand","品牌资源接入"), participants=members,
        messages=listOf(
            ChatMessage("own","","移动端也沿用这套品牌，\n阅读和回复要轻一点。",true,createdAt="2026-09-07T10:24:00+08:00"),
            ChatMessage("product","产品 Leader","收到。导航和群聊分开，\n手机上一次专注一件事。",false,createdAt="2026-09-07T10:25:00+08:00",device="mini1",model="gpt-6"),
            ChatMessage("designer","设计 Worker","三屏稿整理好了，可以先看整体。",false,createdAt="2026-09-07T10:27:00+08:00",device="mini2",model="gpt-6",files=listOf(TextAttachmentUi("notes","zork-mobile-notes.md","# Zork 移动端设计说明", "设计说明 · Markdown"))),
            ChatMessage("illustrator","插画 Worker","头像直接复用，保留已知作者的辨识度。",false,createdAt="2026-09-07T10:28:00+08:00",device="mini2",model="gpt-6")),
        draft=if(route=="composer") "整理一下这些资料。\n先确认范围，再给出方案。\n保留需要我决定的问题。\n附件里是当前要求。" else "",
        attachments=if(route=="composer") listOf(TextAttachmentUi("draft-file","requirements.md","# 当前要求")) else emptyList(),
        comments=listOf(DraftCommentUi("comment","brand","product","产品 Leader",null,"导航和群聊分开","切换后保留阅读位置。")),
        home=fixtureHome())
}
private fun fixtureHome(): HomeNavigation {
    val now=System.currentTimeMillis(); val hour=3_600_000L
    fun chat(peer:String,id:String,title:String,description:String,section:String,at:Long,unread:Boolean=false,archived:Boolean=false)=
        HomeChat(peer,peer,id,title,description,"gpt-6",unread,archived,false,null,3,at,section,true,false)
    return HomeNavigation(listOf(
        chat("mini2","guide","品牌规范整理","把导航和群聊分开","unread",now-2*hour,unread=true),
        chat("mini1","brand","品牌资源接入","头像直接复用","today",now-hour),
        chat("mini2","mobile","移动端交互调研","三屏稿整理好了","today",now-3*hour),
        chat("mini1","offline","离线恢复怎么处理","","week",now-72*hour)),
        listOf(chat("mini1","old","旧版导航","","earlier",now-400*hour,archived=true)),archivedTotal=1,loaded=true)
}
private fun fixtureSettings(): MobileSettingsState {
    val models=org.json.JSONArray().put(obj("id" to "fixture-model","api" to "openai-responses","enabled" to true,"default" to true,"default_thinking" to "off","thinking" to org.json.JSONArray().put("off"),
        "limits" to obj("context_window_tokens" to 128000,"max_output_tokens" to 4096),"capabilities" to obj("input" to org.json.JSONArray().put("text").put("image"))))
        .put(obj("id" to "unconfigured-model","api" to "openai-responses","enabled" to false,"thinking" to org.json.JSONArray().put("off"),"default_thinking" to "off"))
    val agents=listOf(obj("id" to "product","name" to "产品领队","role" to "leader","avatar" to "fox","profile_id" to "studio","model" to "fixture-model","thinking" to "off"),
        obj("id" to "designer","name" to "设计队员","role" to "worker","avatar" to "cat","profile_id" to "studio","model" to "fixture-model","thinking" to "off","allowed_leaders" to org.json.JSONArray().put("other/leader")))
    val profiles=listOf(obj("profile_id" to "studio","name" to "工作室订阅","provider" to "openai","billing" to "subscription","verified" to true,"models" to models,
        "quota" to obj("failed" to false,"windows" to org.json.JSONArray().put(obj("name" to "","minutes" to 300,"remaining" to 72,"resets_at" to 1789002000)),"balance" to org.json.JSONArray().put(0).put("USD")),"checkedAt" to "2026-09-10T01:00:00Z"),
        obj("profile_id" to "research","provider" to "anthropic","billing" to "usage","verified" to false,"models" to org.json.JSONArray()))
    val providers=listOf(obj("id" to "openai","label" to "OpenAI","billing" to org.json.JSONArray().put(obj("id" to "subscription","label" to "ChatGPT 订阅","deviceCode" to true)).put(obj("id" to "usage","label" to "API","deviceCode" to false))),obj("id" to "anthropic","label" to "Anthropic","billing" to org.json.JSONArray().put(obj("id" to "usage","label" to "API","deviceCode" to false))))
    return MobileSettingsState(page="device",device=Peer("mini1","工作室的 MacBook Air",""),fromChat=true,online=true,agents=agents,profiles=profiles,profile=profiles[0],providers=providers,
        info=obj("name" to "工作室的 MacBook Air","station" to obj("release_version" to "0.1.30"),"update" to obj("supported" to false,"reason" to "此设备由客户端管理，可在设备上开启后台运行。")))
}

internal const val literalUserFixture = "# 标题\n**中文🐈** `代码`\n[链接](https://example.test) &amp;\n```rust\nlet x = 1;\n```"
