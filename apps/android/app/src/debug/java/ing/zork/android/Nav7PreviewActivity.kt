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
    /** Tests: show another connection's page, as navigating there would. */
    var openProfile: (String) -> Unit = {}
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); configureZorkSystemBars()
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val route = intent.getStringExtra("screen") ?: "navigation"
        val width = intent.getIntExtra("width",390)
        setContent {
            CompositionLocalProvider(LocalDensity provides if(width>0) Density(1f, 1f) else LocalDensity.current) {
                ZorkTheme(previewTheme) {
                    ZorkPageBackground(lightPage = route != "navigation") {
                        Box((if(width>0) Modifier.requiredSize(width.dp, 844.dp) else Modifier.fillMaxSize().safeDrawingPadding()).onGloballyPositioned {
                            val b = it.boundsInWindow(); contentBounds = Rect(b.left.toInt(), b.top.toInt(), b.right.toInt(), b.bottom.toInt())
                        }) {
                            if (route == "new-chat") {
                                val actions = remember { org.json.JSONArray() }
                                fun project() = JSONObject(NewChatFixtureBridge.render(JSONObject().put("scenario", intent.getStringExtra("scenario") ?: "draft").put("actions", actions).toString()))
                                var snapshot by remember { mutableStateOf(project()) }
                                newChatSnapshot = snapshot
                                NewChatPage(NewChatUi(fixturePeers()[0], snapshot), {}, { action, value ->
                                    lastAction=action
                                    val intent=JSONObject().put("action",action)
                                    if(value != null) intent.put(if(action=="edit" || action=="submit") "text" else "value",value)
                                    actions.put(intent);lastBody=intent;snapshot=project()
                                }, {})
                            }
                            else if (route in listOf("home","appearance","device","models","profile","profile-custom","model-connections","services","notifications","archived")) {
                                var settings by remember { mutableStateOf(if (route == "profile-custom") fixtureSettings().let { it.copy(page="profile",profile=it.profiles.first { p -> p.text("profile_id")=="lab" }) }
                                    else settingsFixturePage(fixtureSettings(), route)) }
                                val resourceTrail = remember { mutableListOf<ResourceSelection>() }
                                var archivedHome by remember { mutableStateOf(fixtureHome()) }
                                openProfile = { id -> settings = settings.copy(page = "profile", profile = settings.profiles.first { it.text("profile_id") == id }) }
                                MobileSettings(settings,fixturePeers(),SettingsActions(back={
                                    if(resourceTrail.isNotEmpty()) {
                                        val selected=resourceTrail.removeAt(resourceTrail.lastIndex)
                                        settings=settings.copy(resource=selected,resourceData=settingsResourceFixture(selected))
                                    } else settings=settings.copy(page=when {
                                        settings.fromConnections && settings.page in listOf("models","profile") -> "model-connections"
                                        settings.page=="profile" -> "models"
                                        else -> "home"
                                    },resource=null,resourceData=null,fromConnections=false,addConnection=false)
                                }, device={settings=settings.copy(page="device",device=it,resource=null,resourceData=null)},
                                    connection={_,profile->lastAction="open-connection:${profile.text("profile_id")}";settings=settings.copy(page="profile",profile=profile,fromConnections=true)},
                                    addConnection={peer->lastAction="add-connection:$peer";settings=settings.copy(page="models",fromConnections=true,addConnection=true)},
                                    theme=previewTheme, saveTheme={previewTheme=it},
                                    clearData={lastAction="clear-data"},
                                    addDevice={lastAction="add-device"},
                                    home=archivedHome, openChat={lastAction="open-chat:${it.optString("_peer")}/${it.text("chat_id")}"},
                                    archiveChat={peer,chat,archived,_->lastAction="archive:$peer/$chat/$archived"
                                        archivedHome=archivedHome.copy(archived=archivedHome.archived.filterNot{it.peer==peer&&it.id==chat},archivedTotal=archivedHome.archivedTotal-1)},
                                    notifications=JSONObject("{\"enabled\":true,\"preview\":false,\"sound\":true,\"background\":false,\"muted\":[]}"),
                                    resource={selected->settings.resource?.let{resourceTrail+=it};settings=settings.copy(resource=selected,resourceData=settingsResourceFixture(selected))},
                                    page={settings=settingsFixturePage(settings,it)},profile={settings=settings.copy(page="profile",profile=it)},perform={action,body->
                                    if(failNextRequest){failNextRequest=false;error("fixture request failed")}
                                    if(action!="available_models"){lastAction=action;lastBody=JSONObject(body.toString())} // a read, not an edit
                                    fun updateProfile(change:(JSONObject)->Unit):JSONObject {
                                        val profile=JSONObject(settings.profile!!.toString());change(profile);lastProfile=JSONObject(profile.toString())
                                        settings=settings.copy(profile=profile,profiles=settings.profiles.map{if(it.text("profile_id")==profile.text("profile_id"))profile else it})
                                        return profile
                                    }
                                    when {
                                    action=="rename_device" -> JSONObject().put("name",body!!.getString("name"))
                                    // Core retitles: a name the user set shows after the account; an empty one clears it.
                                    action=="rename_profile" -> updateProfile { val name=body!!.getString("name").trim()
                                        it.put("name",name.ifBlank{null}).put("custom_name",name.ifBlank{null}) }
                                    action=="enable_model" -> updateProfile { profile ->
                                        profile.optJSONArray("models").objects().find{it.text("id")==body.getString("model")}!!.put("enabled",body!!.getBoolean("enabled"))
                                    }
                                    action=="discover_models" -> discoverFixture(settings.profile!!, settings.profiles, settings.providers).let { (profile, result) ->
                                        lastProfile=profile
                                        settings=settings.copy(profile=profile,profiles=settings.profiles.map{if(it.text("profile_id")==profile.text("profile_id"))profile else it})
                                        result
                                    }
                                    // What the provider lists: the two ids a fetch imports.
                                    action=="available_models" -> JSONObject().put("ids",org.json.JSONArray().put("o3").put("vendor-x-preview"))
                                    action=="remove_model" -> updateProfile { profile ->
                                        val id=body.getJSONObject("model").getString("id")
                                        profile.put("models",org.json.JSONArray(profile.optJSONArray("models").objects().filter{it.text("id")!=id}))
                                    }
                                    action=="save_model" -> updateProfile { profile ->
                                        val result=JSONObject(NativeBridge.previewModels(body.getJSONObject("input").toString(),profile.getJSONArray("models").toString()))
                                        check(result.optBoolean("ok")){result.text("error")}
                                        profile.put("models",result.getJSONObject("data").getJSONArray("models"))
                                    }
                                    action=="save_agent" -> JSONObject()
                                    else -> JSONObject()
                                }}), localDevice = fixtureLocalDevice())
                            }
                            else Workbench(fixture(route), WorkbenchActions(resend = { lastAction = "resend:$it" }, deleteFailed = { lastAction = "delete:$it" }, newChat = { lastAction = "new-chat:${it.id}" }, session = { lastAction = "session:${it.optString("_peer")}/${it.text("chat_id")}" }, device = { lastAction = "device:${it.id}" }))
                        }
                    }
                }
            }
        }
    }
}
/**
 * The provider "reports" o3 and a model the catalog does not know. Like core's
 * discovery, o3 gets its preset values (through the shared editor and save rules)
 * but stays off; the unknown one stays unconfigured. A second fetch finds nothing.
 */
private fun discoverFixture(profile: JSONObject, profiles: List<JSONObject>, providers: List<JSONObject>): Pair<JSONObject, JSONObject> {
    val models = profile.optJSONArray("models") ?: org.json.JSONArray()
    if (models.objects().any { it.text("id") == "o3" }) return profile to obj("added" to 0, "preset" to 0, "pending" to 0, "message" to "没有新模型")
    val context = modelEditorContext(profile, profiles, providers, emptyList())
    val opened = JSONObject(NativeBridge.modelEditor(obj("context" to context, "state" to null, "action" to obj("type" to "open_add")).toString())).getJSONObject("data")
    val settled = JSONObject(NativeBridge.modelEditor(obj("context" to context, "state" to opened.getJSONObject("state"),
        "action" to obj("type" to "settle", "id" to "o3")).toString())).getJSONObject("data")
    val saved = JSONObject(NativeBridge.modelEditor(obj("context" to context, "state" to settled.getJSONObject("state"),
        "action" to obj("type" to "save")).toString())).getJSONObject("data")
    val applied = JSONObject(NativeBridge.previewModels(saved.getJSONObject("effect").getJSONObject("save").toString(), models.toString()))
        .getJSONObject("data").getJSONArray("models")
    applied.objects().first { it.text("id") == "o3" }.put("enabled", false)
    applied.put(obj("id" to "vendor-x-preview", "api" to "openai-responses", "enabled" to false))
    return JSONObject(profile.toString()).put("models", applied) to
        obj("added" to 2, "preset" to 1, "pending" to 1, "message" to "获取到 2 个新模型：1 个已按预设填好，1 个待配置")
}

private fun obj(vararg pairs: Pair<String, Any?>) = JSONObject().apply { pairs.forEach { put(it.first, it.second ?: JSONObject.NULL) } }
private fun leader(id: String, name: String, avatar: String) = obj("id" to id,"name" to name,"avatar" to avatar,"can_open" to true)
private fun task(id: String, title: String, unread: Int = 0) = obj("chat_id" to id,"title" to title,"unread" to (unread > 0),"in_preview" to true)
/** Statuses arrive as core serializes them in the directory snapshot. */
private fun fixturePeers() = listOf(
    Peer("mini1","mini1","", JSONObject("""{"status":{"state":"direct"}}""").deviceStatus()),
    Peer("mini2","mini2","", JSONObject("""{"status":{"state":"offline"}}""").deviceStatus()))
/** This phone as core names it in the directory snapshot's `local`. */
private fun fixtureLocalDevice() = Peer("key:phone","C","", machine="Pixel 8", colorKey="seq:2")
/** Mesh devices as core names them: short display names, machine names kept. */
private fun namedPeers() = listOf(
    Peer("air","A","", JSONObject("""{"status":{"state":"direct"}}""").deviceStatus(), machine="zuozijiandeMacBook-Air", colorKey="seq:0"),
    Peer("studio","B","", JSONObject("""{"status":{"state":"relay"}}""").deviceStatus(), machine="zuozijians-Mac-Studio", colorKey="seq:1"),
    Peer("mini1","mini1","", JSONObject("""{"status":{"state":"offline"}}""").deviceStatus()))
private fun fixture(route: String): WorkbenchState {
    if (route == "device-names") return fixture("navigation").let { it.copy(peers=namedPeers(), activePeer=namedPeers()[0]) }
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
            ChatMessage("illustrator","插画 Worker","头像直接复用，保留已知作者的辨识度。",false,createdAt="2026-09-07T10:28:00+08:00",device="mini2",model="gpt-6",
                deliveredFiles=listOf(ChatFileUi("file-report","首页改版对比.pdf",2_516_582,"application/octet-stream","file","PDF","2.4 MB"),
                    ChatFileUi("file-recording","录屏.mov",18_874_368,"application/octet-stream","file","MOV","18 MB")))),
        draft=if(route=="composer") "整理一下这些资料。\n先确认范围，再给出方案。\n保留需要我决定的问题。\n附件里是当前要求。" else "",
        draftFiles=if(route=="composer") listOf(ChatFileUi("file-draft","requirements.md",14_336,"text/plain","text","MD","14 KB"),
            ChatFileUi("file-shot","首页草图.png",1_258_291,"image/png","image","PNG","1.2 MB",thumbnail=true)) else emptyList(),
        attaching=if(route=="composer") listOf(PendingFileUi("pending-failed","设计稿.sketch","无法读取 · 权限被拒绝")) else emptyList(),
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
    // `title` and `custom_name` are what core derives from `account_label` and the name.
    val lab=obj("profile_id" to "lab","name" to "本地 vLLM","provider" to "openai-compatible","billing" to "usage","verified" to true,
        "account_label" to "···7f3a","title" to "API · ···7f3a","custom_name" to "本地 vLLM",
        "models" to org.json.JSONArray().put(obj("id" to "qwen3-32b","api" to "openai-completions","enabled" to true,"thinking" to org.json.JSONArray().put("off").put("high"),"default_thinking" to "high",
            "limits" to obj("context_window_tokens" to 128000,"max_output_tokens" to 16000),"capabilities" to obj("input" to org.json.JSONArray().put("text")))))
    val profiles=listOf(obj("profile_id" to "studio","name" to "工作室订阅","provider" to "openai","billing" to "subscription","verified" to true,"models" to models,
        "account_label" to "studio@example.test","title" to "studio@example.test","custom_name" to "工作室订阅","access" to "ChatGPT 订阅",
        "quota" to obj("failed" to false,"windows" to org.json.JSONArray().put(obj("name" to "","minutes" to 300,"remaining" to 72,"resets_at" to 1789002000)),"balance" to org.json.JSONArray().put(0).put("USD")),"checkedAt" to "2026-09-10T01:00:00Z"),
        obj("profile_id" to "research","provider" to "anthropic","billing" to "usage","verified" to false,"models" to org.json.JSONArray(),
            "account_label" to "···k9x2","title" to "API · ···k9x2","custom_name" to null), lab)
    val providers=listOf(obj("id" to "openai","label" to "OpenAI","billing" to org.json.JSONArray().put(obj("id" to "subscription","label" to "ChatGPT 订阅","deviceCode" to true)).put(obj("id" to "usage","label" to "API","deviceCode" to false))),obj("id" to "anthropic","label" to "Anthropic","billing" to org.json.JSONArray().put(obj("id" to "usage","label" to "API","deviceCode" to false))),
        obj("id" to "openai-compatible","label" to "OpenAI 兼容","billing" to org.json.JSONArray().put(obj("id" to "usage","label" to "API","deviceCode" to false,
            "template" to obj("models" to org.json.JSONArray().put(obj("api" to "openai-completions")))))))
    val failedProfile=obj("profile_id" to "router","name" to "OpenRouter","provider" to "openrouter","billing" to "usage","verified" to false,"verification" to "failed","models" to org.json.JSONArray(),
        "account_label" to "···r0ut","title" to "OpenRouter · ···r0ut","custom_name" to null)
    // The same OpenCode Go key on both devices: core shows it once, quota from the fresher sample.
    fun go(id: String, remaining: Int, at: String, models: List<String>) = obj("profile_id" to id,"name" to "OpenCode-Go","provider" to "opencode-go","billing" to "subscription",
        "verified" to true,"verification" to "verified","account_key" to "opencode-go:k:0123456789abcdef","checkedAt" to at,
        "account_label" to "···a1b2","title" to "OpenCode Go 订阅 · ···a1b2","custom_name" to null,
        "models" to org.json.JSONArray(models.map { obj("id" to it) }),
        "quota" to obj("failed" to false,"windows" to org.json.JSONArray().put(obj("name" to "","minutes" to 300,"remaining" to remaining))))
    val goA=go("OpenCode-Go",64,"2026-09-10T01:00:00Z",listOf("glm-5.1","kimi-k2.6"))
    val goB=go("go-mini2",58,"2026-09-10T01:20:00Z",listOf("glm-5.1","deepseek-flash"))
    val openCode=obj("id" to "opencode-go","label" to "OpenCode Go","billing" to org.json.JSONArray().put(obj("id" to "subscription","label" to "OpenCode Go 订阅","deviceCode" to false)))
    val mini1Profiles=profiles.map { JSONObject(it.toString()).put("verification", if (it.optBoolean("verified")) "verified" else "pending") } + goA
    val connections=listOf(
        obj("peer" to "mini1","name" to "mini1","state" to "ready","cached" to false,"profiles" to org.json.JSONArray(mini1Profiles),"providers" to org.json.JSONArray(providers + openCode)),
        obj("peer" to "mini2","name" to "mini2","state" to "failed","error" to "连接超时","cached" to true,"loaded_at_ms" to System.currentTimeMillis() - 12 * 60_000,
            "profiles" to org.json.JSONArray().put(failedProfile).put(goB),"providers" to org.json.JSONArray().put(obj("id" to "openrouter","label" to "OpenRouter")).put(openCode)))
    fun source(peer: String, profile: JSONObject) = obj("peer" to peer,"device" to peer,"profile" to profile)
    fun single(peer: String, profile: JSONObject) = obj("id" to "profile:$peer/${profile.text("profile_id")}","name" to connectionTitle(profile),
        "title" to connectionTitle(profile),"custom_name" to customName(profile),"access" to profile.opt("access"),"provider" to profile.text("provider"),
        "peer" to peer,"profile" to profile,"models" to (profile.optJSONArray("models")?.length() ?: 0),"sources" to org.json.JSONArray().put(source(peer, profile)))
    // Core's `accounts` for these devices (see zork-client-core model_connections).
    val accounts=mini1Profiles.dropLast(1).map { single("mini1", it) } + single("mini2", failedProfile) +
        obj("id" to "account:opencode-go:k:0123456789abcdef","name" to "OpenCode Go 订阅 · ···a1b2","title" to "OpenCode Go 订阅 · ···a1b2","custom_name" to null,"provider" to "opencode-go","peer" to "mini2","profile" to goB,"models" to 3,
            "sources" to org.json.JSONArray().put(source("mini1", goA)).put(source("mini2", goB)))
    return MobileSettingsState(page="device",device=Peer("mini1","B","",machine="工作室的 MacBook Air"),fromChat=true,online=true,agents=agents,profiles=profiles,profile=profiles[0],providers=providers,connections=connections,accounts=accounts,
        info=obj("name" to "工作室的 MacBook Air","station" to obj("release_version" to "0.1.30"),"update" to obj("supported" to false,"reason" to "此设备由客户端管理，可在设备上开启后台运行。")))
}

internal const val literalUserFixture = "# 标题\n**中文🐈** `代码`\n[链接](https://example.test) &amp;\n```rust\nlet x = 1;\n```"
