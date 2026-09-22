/* Product snapshot: crates/profile/src/providers + desktop/profiles.rs. No live auth. */
'use strict';
const profileMode=(id,label,auth,models)=>({id,label,auth,models:models.map(id=>({id}))});
const profileCatalog=[
 {id:'xai',label:'xAI / Grok',billing:[profileMode('subscription','Grok 订阅','device',['grok-4.6']),profileMode('usage','API 按量','key',['grok-4.6'])]},
 {id:'openai',label:'OpenAI',billing:[profileMode('usage','API 按量','key',['gpt-4.1']),profileMode('subscription','ChatGPT 订阅 (Codex)','device',['gpt-5.6-luna'])]},
 {id:'openai-compatible',label:'OpenAI Compatible',billing:[profileMode('usage','Custom API','key',[])]},
 {id:'opencode-go',label:'OpenCode Go',billing:[profileMode('subscription','OpenCode Go 订阅','key',['muse-spark-1.2-contributor'])]},
 {id:'anthropic',label:'Anthropic / Claude',billing:[profileMode('usage','API 按量','key',['claude-sonnet-4-6']),profileMode('subscription','Claude 订阅 (Pro/Max)','callback',['claude-sonnet-4-6'])]},
 {id:'github-copilot',label:'GitHub Copilot',billing:[profileMode('subscription','Copilot 订阅','device',['gpt-4.1']),profileMode('usage','API 按量','key',['gpt-4.1'])]},
 {id:'openrouter',label:'OpenRouter',billing:[profileMode('usage','API 按量','callback',['openrouter/auto','anthropic/claude-sonnet-4'])]},
 {id:'kimi-coding',label:'Kimi For Coding',billing:[profileMode('subscription','Kimi Code 订阅','device',['kimi-for-coding','kimi-k2.5']),profileMode('usage','API 按量','key',['kimi-for-coding','kimi-k2.5'])]}
];
const providerLogoNames={'xai':'xai','openai':'openai','openai-compatible':'compatible','opencode-go':'opencode','anthropic':'anthropic','github-copilot':'githubcopilot','openrouter':'openrouter','kimi-coding':'kimi'};
const providerLogo=id=>`<img class="provider-logo" src="gui/provider-icons/${providerLogoNames[id]}.svg" alt="">`;
let profileDraft=null;
function addProfileDialog(){profileDraft={node:settingsDevice,provider:'xai',billing:'subscription',name:'',url:'',authPreview:false};renderProfileDialog();}
function selectedProfileMode(){const provider=profileCatalog.find(p=>p.id===profileDraft.provider);return {provider,mode:provider.billing.find(b=>b.id===profileDraft.billing)};}
function renderProfileDialog(){
 const {provider,mode}=selectedProfileMode(),custom=provider.id==='openai-compatible',busy=profileDraft.authPreview;
 const available=profileCatalog.filter(p=>p.billing.some(b=>b.id===profileDraft.billing));
 const selectedProvider=`${providerLogo(provider.id)}<span>${provider.label}</span>`;
 modal('添加模型连接',`<form id="add-profile-form"><p class="profile-device-note">${icon('node')} ${profileDraft.node} · 连接保存在此设备</p><div class="field"><span>接入方式</span><div class="profile-billing" role="group" aria-label="接入方式">${[['subscription','订阅账号'],['usage','API 接入']].map(([id,label])=>`<button type="button" data-action="profile-billing" data-billing="${id}" aria-pressed="${id===mode.id}" ${busy?'disabled':''}>${label}</button>`).join('')}</div></div><div class="field"><span>供应商</span>${busy?`<div class="profile-provider-selected">${selectedProvider}</div>`:`<details class="profile-provider-dropdown" id="profile-provider-menu"><summary aria-label="选择供应商，当前 ${provider.label}">${selectedProvider}${icon('chevron-down')}</summary><div class="profile-provider-options" role="group" aria-label="供应商选项">${available.map(p=>`<button type="button" data-action="profile-provider" data-provider="${p.id}" aria-pressed="${p.id===provider.id}">${providerLogo(p.id)}<span>${p.label}</span>${p.id===provider.id?icon('check'):''}</button>`).join('')}</div></details>`}<small class="profile-mode-note">${mode.label}</small></div><label class="field"><span>连接名称</span><input data-profile-field="name" value="${esc(profileDraft.name)}" placeholder="例如：my-${provider.id}" maxlength="60" required ${busy?'readonly':''}></label>${custom?`<label class="field"><span>接口地址</span><input data-profile-field="url" type="url" value="${esc(profileDraft.url)}" placeholder="https://…/v1" required></label>`:''}${mode.auth==='key'?'<label class="field"><span>API Key</span><input id="profile-key-preview" type="password" autocomplete="off" placeholder="样稿无需填写真实密钥"></label>':''}${busy?`<div class="profile-auth-preview"><b>${mode.auth==='callback'?'浏览器授权':'设备码登录'}</b><p>${mode.auth==='callback'?'在浏览器完成登录后，粘贴返回的授权码或地址。':'在浏览器输入设备码，完成授权后连接自动保存。'}</p>${mode.auth==='callback'?'<input placeholder="浏览器返回内容 · 流程预览" disabled>':'<div class="profile-device-code">设备码由已连接的节点生成</div>'}<small>流程预览，尚未发起真实登录。</small></div>`:''}<p id="profile-error" class="profile-error" role="alert" hidden></p><p class="model-form-note">连接后再管理可用模型。样稿不提交凭据或发起真实登录。</p><div class="modal-actions"><button class="button" type="button" data-action="${busy?'profile-cancel-auth':'close-dialog'}">${busy?'取消登录':'取消'}</button><button class="button primary" type="submit">${busy?'模拟完成登录':mode.auth==='key'?'保存连接':'登录并连接'}</button></div></form>`);
}
function profileError(message){$('profile-error').hidden=false;$('profile-error').textContent=message;}
function saveProfilePreview(){
 const {provider,mode}=selectedProfileMode(),custom=provider.id==='openai-compatible';
 const name=profileDraft.name.trim();if(!name){profileError('请填写连接名称');return;}
 if(profiles.some(p=>p.node===profileDraft.node&&p.name===name)){profileError('这台设备已有同名连接，请使用其他名称');return;}
 if(custom&&!/^https?:\/\//.test(profileDraft.url.trim())){profileError('请填写有效接口地址');return;}
 const models=[];
 const node=profileDraft.node;
 const saved={id:'profile-'+(++sequence),profile_id:name,name,node,provider:provider.id,billing:mode.id,authKind:mode.auth,models,...(custom?{url:profileDraft.url.trim()}:{}),verified:false};profiles.push(saved);
 if($('profile-key-preview'))$('profile-key-preview').value='';
 profileDraft=null;$('chat-dialog').close();openSettings('models',node);viewProfile(saved.id);
}
function currentProfile(id){return profiles.find(p=>p.id===id&&p.node===settingsDevice);}
function viewProfile(id){
 const profile=currentProfile(id);if(!profile)return;const provider=profileCatalog.find(p=>p.id===profile.provider);
 modal(profile.name,`<p class="profile-device-note">${providerLogo(profile.provider)}${profile.node} · ${esc(provider?.label||profile.provider)} · 未验证</p><div class="profile-model-toolbar"><h3>模型</h3><div><button class="button" data-action="fetch-profile-models" data-profile="${id}">获取模型</button><button class="button" data-action="add-profile-model" data-profile="${id}">${icon('plus')}手动添加</button></div></div><p id="profile-model-feedback" class="model-form-note" role="status" hidden></p><div class="profile-detail-models">${profile.models.length?profile.models.map(model=>`<div><b>${esc(model.id)}</b><small>手动添加 · 未验证${model.limits?` · 上下文 ${model.limits.context_window_tokens} / 输出 ${model.limits.max_output_tokens}`:''}</small></div>`).join(''):'<p class="model-empty">暂无模型。连接后可获取供应商支持的模型，也可以手动添加。</p>'}</div><p class="model-form-note">在 Agent 设置中选择此连接，再选择其中的模型。</p>`);
}
function addProfileModelDialog(id){
 const profile=currentProfile(id);if(!profile)return;
 modal('添加模型',`<form id="profile-model-form" data-profile="${id}"><p class="profile-device-note">${profile.node} · ${esc(profile.name)}</p><label class="field"><span>模型 ID</span><input name="model" maxlength="150" placeholder="供应商提供的模型标识" required></label><details class="profile-capacity"><summary>模型容量</summary><div><label class="field"><span>上下文 token 上限</span><input name="context" type="number" min="1" value="32000" required></label><label class="field"><span>输出 token 上限</span><input name="output" type="number" min="1" value="4096" required></label></div></details><p id="profile-model-error" class="profile-error" role="alert" hidden></p><p class="model-form-note">手动填写的配置不会自动验证模型是否可用。</p><div class="modal-actions"><button class="button" type="button" data-action="view-profile" data-profile="${id}">返回连接</button><button class="button primary" type="submit">添加模型</button></div></form>`);
}
function updateAgentModelOptions(){
 const selector=$('agent-profile');if(!selector)return;
 const form=$('agent-settings-form'),profile=profiles.find(p=>p.id===selector.value&&p.node===form.dataset.device);
 const modelSelect=$('agent-model');modelSelect.innerHTML=profile?.models.length?profile.models.map(m=>`<option value="${esc(m.id)}">${esc(m.id)}</option>`).join(''):`<option value="">${profile?'此连接尚未添加模型':'先选择模型连接'}</option>`;modelSelect.disabled=!profile?.models.length;
}
document.addEventListener('click',e=>{
 const b=e.target.closest('[data-action]');if(!b)return;
 if(b.dataset.action==='profile-provider'&&profileDraft&&!profileDraft.authPreview){const p=profileCatalog.find(p=>p.id===b.dataset.provider&&p.billing.some(mode=>mode.id===profileDraft.billing));if(!p)return;profileDraft.provider=p.id;profileDraft.url='';renderProfileDialog();$('profile-provider-menu').querySelector('summary').focus();}
 else if(b.dataset.action==='profile-billing'&&profileDraft&&!profileDraft.authPreview){profileDraft.billing=b.dataset.billing;const eligible=profileCatalog.filter(p=>p.billing.some(mode=>mode.id===profileDraft.billing));if(!eligible.some(p=>p.id===profileDraft.provider)){profileDraft.provider=eligible[0].id;profileDraft.url='';}renderProfileDialog();}
 else if(b.dataset.action==='profile-cancel-auth'){profileDraft.authPreview=false;renderProfileDialog();}
 else if(b.dataset.action==='view-profile')viewProfile(b.dataset.profile);
 else if(b.dataset.action==='add-profile-model')addProfileModelDialog(b.dataset.profile);
 else if(b.dataset.action==='fetch-profile-models'){const feedback=$('profile-model-feedback');if(feedback){feedback.hidden=false;feedback.textContent='样稿未连接真实节点，暂时无法获取。可以先手动添加模型。';}}
});
document.addEventListener('keydown',e=>{const menu=$('profile-provider-menu');if(!menu)return;if(e.key==='Escape'&&menu.open){menu.open=false;menu.querySelector('summary').focus();}if(e.key==='ArrowDown'&&e.target===menu.querySelector('summary')){e.preventDefault();menu.open=true;menu.querySelector('[data-provider]')?.focus();}});
document.addEventListener('input',e=>{const field=e.target.dataset.profileField;if(field&&profileDraft)profileDraft[field]=e.target.value;});
document.addEventListener('change',e=>{if(e.target.id==='agent-profile')updateAgentModelOptions();});
document.addEventListener('submit',e=>{
 if(e.target.id!=='add-profile-form')return;e.preventDefault();if(!profileDraft)return;
 if(!profileDraft.name.trim()){profileError('请填写连接名称');return;}
 const {mode}=selectedProfileMode();
 if(mode.auth!=='key'&&!profileDraft.authPreview){profileDraft.authPreview=true;renderProfileDialog();return;}
 saveProfilePreview();
});

document.addEventListener('submit',e=>{
 if(e.target.id!=='profile-model-form')return;e.preventDefault();
 const profile=currentProfile(e.target.dataset.profile);if(!profile)return;
 const data=new FormData(e.target),id=String(data.get('model')||'').trim(),context=Number(data.get('context')),output=Number(data.get('output'));
 const fail=message=>{$('profile-model-error').hidden=false;$('profile-model-error').textContent=message;};
 if(!id){fail('请填写模型 ID');return;}
 if(profile.models.some(model=>model.id===id)){fail('此连接已包含该模型');return;}
 if(!Number.isInteger(context)||!Number.isInteger(output)||output<=0||context<output){fail('输出 token 上限必须大于 0，且不能超过上下文上限');return;}
 profile.models.push({id,source:'manual',limits:{context_window_tokens:context,max_output_tokens:output}});renderSettings();viewProfile(profile.id);
});
