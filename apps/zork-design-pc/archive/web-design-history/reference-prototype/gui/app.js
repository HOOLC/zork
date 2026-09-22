/* Local-only prototype. All task mutations stay in this page's memory. */
'use strict';
const $ = id => document.getElementById(id);
const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const common = new Set(['search','plus','chevron-down','arrow-left','arrow-up','x','settings','download','inbox','bars-three','panel-left','paperclip','check']);
const icon = (name,extra='') => `<span class="icon ${extra}" style="--icon:url(/${common.has(name)?'design/reference-prototype/gui/glyphs/':'design/reference-prototype/svg/icons/'}${name}.svg)" aria-hidden="true"></span>`;
const avatar = (name,cls='',alt='') => `<img class="${cls}" src="svg/avatars/${name}.svg" alt="${escapeHtml(alt)}">`;
const avatarIds=['cat','bunny','bear','fox','panda','chick','dog','owl','koala','penguin','deer','octopus'];
const iconIds=['node','leader','worker','mesh','task','handoff','result','review','history','workspace','permission','offline'];
const leaders={
  product:{name:'产品 Leader',avatar:'fox',node:'mini1',description:'把想法整理成目标，和你一起推进产品。',worker:'owl',workerName:'设计 Worker'},
  engineering:{name:'工程 Leader',avatar:'dog',node:'mini1',description:'关注实现、稳定性与可维护性。',worker:'cat',workerName:'工程 Worker'},
  research:{name:'研究 Leader',avatar:'owl',node:'mini2',description:'梳理信息与证据，形成可以使用的结论。',worker:'deer',workerName:'研究 Worker'}
};
const statusInfo={review:['待验收','review'],working:['执行中','working'],blocked:['需要你决定','attention'],completed:['已完成','completed'],open:['待安排','open'],cancelled:['已取消','cancelled']};
const badge=(state,pill=false)=>`<span class="status ${state}${pill?' pill':''}">${icon(statusInfo[state][1])}${statusInfo[state][0]}</span>`;
const files={
  icons:{name:'zork-interface-icons.zip',caption:'12 个功能图标 · SVG',href:'downloads/zork-interface-icons.zip',kind:'icons',task:'品牌资源接入'},
  avatars:{name:'zork-avatars-12.zip',caption:'12 个头像 · SVG + PNG',href:'downloads/zork-avatars-12.zip',kind:'avatars',task:'品牌资源接入'},
  guide:{name:'brand-guidelines.md',caption:'品牌、状态与资源规范',href:'proposal.md',kind:'guide',task:'整理品牌规范'}
};
const initialTasks=[
  {id:'brand',title:'品牌资源接入',goal:'统一图标和头像，形成可复用的桌面品牌资源。',state:'review',leader:'product',worker:'owl',workerName:'设计 Worker',node:'mini2',excerpt:'图标和头像已整理，等待确认',age:'刚刚',version:1,files:['icons','avatars'],discussion:[['user','把图标和头像整理成一套可复用的资源，保持 macOS 的原生感。'],['worker','已按统一的比例、圆角和视觉重量整理好首版资源。你可以先查看交付，再决定是否接受这一版。']]},
  {id:'retry',title:'确认离线重试策略',goal:'让远端断线后的任务恢复可预期，避免重复执行有副作用的操作。',state:'blocked',leader:'engineering',worker:'dog',workerName:'工程 Worker',node:'mini1',excerpt:'需要确认断线后的重试方式',age:'稍早',version:0,files:[],discussion:[['user','连接中断以后，任务应该怎么继续？'],['worker','两种恢复方式都可以实现。需要你先确认默认策略，我再继续处理。']]},
  {id:'guide',title:'整理品牌规范',goal:'整理颜色、头像、图标和任务状态的统一使用规则。',state:'review',leader:'product',worker:'deer',workerName:'整理 Worker',node:'mini2',excerpt:'品牌规范已整理，等待验收',age:'稍早',version:1,files:['guide'],discussion:[['user','把这次的设计决定整理成以后能复用的规范。'],['worker','已经整理为一份文档，包括身份、状态、资源格式和后续接入顺序。']]},
  {id:'offline',title:'优化离线恢复体验',goal:'设备暂时离线时，保留已同步的任务、消息和文件。',state:'working',leader:'engineering',worker:'cat',workerName:'客户端 Worker',node:'mini1',excerpt:'正在检查断线与恢复后的状态',age:'进行中',version:0,files:[],discussion:[['user','断网之后不要把已经看到的内容清空。'],['worker','正在梳理连接状态、已同步内容和待发送消息的区别。']]},
  {id:'optical',title:'检查图标的小尺寸表现',goal:'检查 16、20、24px 下的轮廓、线条与辨识度。',state:'working',leader:'product',worker:'koala',workerName:'视觉 Worker',node:'mini2',excerpt:'正在检查各尺寸下的视觉重量',age:'进行中',version:0,files:[],discussion:[['user','小尺寸时也要清楚，不能只是把大图缩小。'],['worker','正在对齐每个符号的光学边界与实际显示尺寸。']]},
  {id:'avatar',title:'绘制 12 个 Agent 头像',goal:'用同一套风格，为不同 Agent 提供可辨认的身份。',state:'completed',leader:'product',worker:'panda',workerName:'插画 Worker',node:'mini2',excerpt:'12 个角色，已接受当前版本',age:'已交付',version:1,files:['avatars'],discussion:[['user','头像也按新风格重新出，先出 12 个。'],['worker','12 个头像已整理，均提供 SVG 与 PNG。']]}
];
let tasks=structuredClone(initialTasks);
let view={mode:'attention',task:'brand',tab:'delivery',leader:'product',query:''};
let nodes={mini1:true,mini2:true};
let chats={product:[],engineering:[],research:[]};
let notificationTimer;

function filteredTasks(){
  return tasks.filter(t=>{
    const inView=view.mode==='attention'?['review','blocked'].includes(t.state):view.mode==='working'?['working','open'].includes(t.state):view.mode==='leader'?t.leader===view.leader:true;
    return inView && (t.title+' '+t.goal+' '+t.workerName).toLowerCase().includes(view.query.toLowerCase());
  });
}
function currentTask(){return tasks.find(t=>t.id===view.task);}
function flash(text){clearTimeout(notificationTimer);$('notice').textContent=text;$('notice').classList.add('visible');notificationTimer=setTimeout(()=>$('notice').classList.remove('visible'),2400);}
function showNav(){document.body.classList.toggle('nav-open');}
function closeNav(){document.body.classList.remove('nav-open');}
function selectTask(id){view.task=id;view.tab=['review','completed'].includes(currentTask()?.state)?'delivery':'discussion';if(view.mode==='leader')view.mode='all';document.body.classList.remove('mobile-list');closeNav();render();}
function selectMode(mode,leader){view.mode=mode;if(leader)view.leader=leader;view.query='';$('task-search').value='';const matches=filteredTasks();view.task=matches[0]?.id??null;view.tab=['review','completed'].includes(currentTask()?.state)?'delivery':'discussion';closeNav();document.body.classList.remove('mobile-list');render();}

function renderNav(){
  const counts={attention:tasks.filter(t=>['review','blocked'].includes(t.state)).length,working:tasks.filter(t=>['working','open'].includes(t.state)).length,all:tasks.length};
  $('primary-nav').innerHTML=[['attention','review','待我处理'],['working','working','正在推进'],['all','task','全部任务'],['files','workspace','交付文件']].map(([key,symbol,label])=>`<button class="nav-row ${view.mode===key?'selected':''}" data-action="mode" data-mode="${key}" aria-current="${view.mode===key?'page':'false'}">${icon(symbol)}<span>${label}</span>${key!=='files'?`<span class="nav-count">${counts[key]}</span>`:''}</button>`).join('');
  $('leader-nav').innerHTML=Object.entries(leaders).map(([key,l])=>`<button class="nav-row leader-row ${view.mode==='leader'&&view.leader===key?'selected':''}" data-action="leader" data-leader="${key}">${avatar(l.avatar,'nav-avatar')}<span>${l.name}</span></button>`).join('');
  $('app-shell').className='app-shell mode-'+view.mode;
  const connectionDot=document.querySelector('.resource-entry .online-dot');
  connectionDot.style.background=Object.values(nodes).some(Boolean)?'var(--color-green)':'var(--color-subtle)';
}
function renderList(){
  const titles={attention:'待我处理',working:'正在推进',all:'全部任务',files:'交付文件',leader:'关联任务'};
  const subtitles={attention:'让值得你注意的事情，先被看见。',working:'已交给伙伴的事，在这里继续。',all:'目标、进展和结果，都有迹可循。',leader:'这位 Leader 正在帮你推进的事。'};
  $('list-heading').textContent=titles[view.mode];$('list-intro').textContent=subtitles[view.mode]||'';
  const rows=filteredTasks();$('list-total').textContent=rows.length+' 项';
  $('task-list').innerHTML=rows.length?rows.map(t=>`<button class="task-item ${view.task===t.id&&view.mode!=='leader'?'selected':''}" data-action="task" data-task="${t.id}" aria-label="${escapeHtml(t.title)}，${statusInfo[t.state][0]}"><span class="task-title-row">${avatar(t.worker)}<span class="task-title">${escapeHtml(t.title)}</span></span><span class="task-meta">${badge(t.state)}<span class="task-age">${t.age}</span></span><p class="task-excerpt">${escapeHtml(t.excerpt)}</p></button>`).join(''):`<div class="empty-list">${view.query?'没有找到匹配的任务':'这里暂时没有任务'}</div>`;
  $('list-foot').textContent=view.mode==='leader'?'长期对话保留上下文，任务各自推进。':'任务跟着目标走，设备在背后协作。';
}
function topBar(label,avatarId){return `<div class="detail-topline"><div class="breadcrumb"><button class="icon-button mobile-back" data-action="show-list" aria-label="返回任务列表">${icon('arrow-left')}</button><button class="icon-button mobile-only" data-action="toggle-nav" aria-label="打开导航">${icon('bars-three')}</button>${avatarId?avatar(avatarId):''}<span>${escapeHtml(label)}</span></div><div class="top-actions"><button class="icon-button" data-action="new-goal" aria-label="新目标" title="新目标">${icon('plus')}</button><button class="icon-button" data-action="resources" aria-label="设备与模型" title="设备与模型">${icon('settings')}</button></div></div>`;}
function fileRow(id){const f=files[id];return `<div class="file-row"><button class="file-open" data-action="file" data-file="${id}"><span class="file-type">${icon('result')}</span><span class="file-info"><b>${f.name}</b><small>${f.caption}</small></span></button><a class="download-link" href="${f.href}" download aria-label="下载 ${f.name}" title="下载">${icon('download')}</a></div>`;}
function previewBlock(task){
  const hasIcons=task.files.includes('icons'),hasAvatars=task.files.includes('avatars');
  if(!hasIcons&&!hasAvatars)return `<div class="decision-box guide-text"><h4>品牌资源使用规范</h4><p>产品标志、Agent 身份和任务状态各自有明确职责。</p><ul><li>统一颜色、字体与图形比例。</li><li>保持 Agent 头像与角色归属一致。</li><li>区分结果提交、执行结束与用户验收。</li><li>按实际用途交付 SVG、PNG 与设计 token。</li></ul></div>`;
  return `<div class="asset-preview">${hasIcons?`<div class="preview-label"><span>功能图标 · 12 个</span><button data-action="file" data-file="icons">查看全部</button></div><div class="glyph-strip">${['node','leader','worker','task','review','workspace'].map(i=>icon(i)).join('')}</div>`:''}${hasAvatars?`<div class="preview-label"><span>Agent 头像 · 12 个</span><button data-action="file" data-file="avatars">查看全部</button></div><div class="avatar-strip">${avatarIds.map(id=>avatar(id)).join('')}</div>`:''}</div>`;
}
function delivery(task){
  if(!task.files.length)return `<div class="empty-delivery">${icon('review')}<h3>还没有提交交付</h3><p>讨论和过程仍可查看，结果准备好后会出现在这里。</p><button class="button" data-action="tab" data-tab="discussion">继续讨论</button></div>`;
  const previous=!['review','completed'].includes(task.state);
  return `<div class="delivery">${task.state==='completed'?`<div class="approved-note">${icon('completed')}你已接受当前交付，结果和文件继续保留。</div>`:previous?'<div class="previous-note">这里保留上一版交付，等待修改后的新结果。</div>':''}<div class="author-row">${avatar(task.worker)}<div><b>${task.workerName}</b><small>提交了版本 ${task.version}</small></div></div><h3>${previous?'上一版交付，留作对照':task.id==='guide'?'把设计决定，整理成可复用的规范':'这版资源已经准备好了'}</h3><p class="intro">${task.id==='guide'?'身份、状态和资源格式已分别整理。以后增加新的图标或头像时，可以沿用同一套规则。':'功能图标和头像采用统一的比例与圆角，所有资源保留可编辑源文件。'}</p>${previewBlock(task)}<div class="file-list">${task.files.map(fileRow).join('')}</div></div>`;
}
function message(who,text,task,leaderOnly=false){
  const l=leaders[leaderOnly?view.leader:task.leader];
  return who==='user'?`<div class="message user"><div class="message-content"><p>${escapeHtml(text)}</p></div></div>`:`<div class="message">${avatar(leaderOnly?l.avatar:task.worker)}<div class="message-content"><div class="message-title">${leaderOnly?l.name:task.workerName}</div><p>${escapeHtml(text)}</p></div></div>`;
}
function composer(type){return `<form class="composer" id="${type==='leader'?'leader-form':'discussion-form'}"><textarea name="message" required maxlength="2000" placeholder="${type==='leader'?'想先推进哪件事？':'补充要求，或继续讨论…'}" aria-label="${type==='leader'?'给 Leader 的消息':'任务讨论消息'}"></textarea><div class="composer-tools"><small>${type==='leader'?'在同一段长期对话里，接着推进。':'讨论围绕当前任务，交付保持独立。'}</small><button class="send-button" type="submit" aria-label="发送消息">${icon('arrow-up')}</button></div></form>`;}
function discussion(task){
  return `<div class="discussion">${task.discussion.map(([who,text])=>message(who,text,task)).join('')}${task.state==='blocked'?`<form class="decision-box" id="decision-form"><h4>断线后，默认怎么继续？</h4><label class="decision-option"><input type="radio" name="policy" value="pause" checked><span><b>先暂停，确认后继续</b><small>遇到执行结果不确定的情况，交给你决定。</small></span></label><label class="decision-option"><input type="radio" name="policy" value="safe"><span><b>仅自动重试可安全重复的操作</b><small>涉及写入或外部操作时，仍然先暂停。</small></span></label><button class="button primary" type="submit">确认，继续推进</button></form>`:''}${composer('discussion')}</div>`;
}
function history(task){
  const events=[['task','接住目标',task.goal,leaders[task.leader].name],['handoff','安排执行',`${task.workerName} 在 ${task.node} 处理任务。`,'任务始终保留原有归属']];
  if(task.state==='blocked')events.push(['attention','等待你的决定','需要确认默认恢复策略。','任务保留在“待我处理”']);
  else if(task.files.length)events.push(['result','提交交付',`提交了版本 ${task.version}，包含 ${task.files.length} 份交付文件。`,'交付并不等于验收完成']);
  else events.push(['working','继续推进','执行过程有新的进展时，会保留在这里。','当前尚未提交交付']);
  if(task.state==='completed')events.push(['completed','接受当前版本',`你已验收版本 ${task.version}。`,'后续可以重新打开任务']);
  return `<div class="history">${events.map(([i,title,body,detail])=>`<div class="event"><div class="event-icon">${icon(i)}</div><div><h4>${title}</h4><p>${escapeHtml(body)}</p><small>${detail}</small></div></div>`).join('')}</div>`;
}
function renderTask(){
  const t=currentTask();if(!t){$('detail-header').innerHTML=topBar('工作台');$('detail-scroll').innerHTML=`<div class="screen-empty"><img src="svg/mark.svg" alt=""><h2>这一刻，没有需要处理的事。</h2><p>可以看看正在推进的任务，或和 Leader 开始一个新目标。</p><button class="button" data-action="new-goal">新目标</button></div>`;$('detail-footer').innerHTML='';return;}
  const l=leaders[t.leader];
  $('detail-header').innerHTML=topBar(l.name,l.avatar)+`<div class="task-heading"><div class="task-heading-title"><h2>${escapeHtml(t.title)}</h2>${badge(t.state,true)}</div><div class="task-context">${avatar(t.worker)}<span>${t.workerName}</span><span class="context-sep">·</span><button data-action="resources">${t.node} 执行${nodes[t.node]?'':' · 暂时离线'}</button></div><div class="goal-line"><span>目标</span><p>${escapeHtml(t.goal)}</p></div></div><div class="tabs" role="tablist" aria-label="任务内容">${[['delivery','交付'],['discussion','讨论'],['history','过程']].map(([key,label])=>`<button class="tab ${view.tab===key?'selected':''}" role="tab" aria-selected="${view.tab===key}" data-action="tab" data-tab="${key}">${label}${key==='delivery'&&t.files.length?`<span class="count">${t.files.length}</span>`:''}</button>`).join('')}</div>`;
  $('detail-scroll').innerHTML=view.tab==='delivery'?delivery(t):view.tab==='discussion'?discussion(t):history(t);
  $('detail-footer').innerHTML=t.state==='review'?`<div class="decision-footer"><span class="version-note">${icon('result')}当前交付：版本 ${t.version}</span><div class="footer-actions"><button class="button ghost" data-action="revise">继续修改</button><button class="button primary" data-action="accept">接受这一版</button></div></div>`:t.state==='completed'?`<div class="decision-footer"><span class="version-note">${icon('completed')}已验收当前结果</span><div class="footer-actions"><button class="button" data-action="reopen">重新打开任务</button></div></div>`:`<div class="decision-footer"><span class="version-note">${icon(t.state==='blocked'?'attention':'working')}${t.state==='blocked'?'等待你的决定':'交付准备好后，再确认结果'}</span><div class="footer-actions"><button class="button ghost" data-action="tab" data-tab="${t.state==='blocked'?'discussion':'history'}">${t.state==='blocked'?'查看需要决定的事':'查看过程'}</button></div></div>`;
}
function renderFiles(){
  $('detail-header').innerHTML=topBar('交付文件');$('detail-footer').innerHTML='';
  $('detail-scroll').innerHTML=`<div class="files-page"><h2>交付文件</h2><p>成果保留在任务里，也可以从这里快速找到。</p><div class="files-grid">${Object.entries(files).map(([id,f])=>`<article class="file-card"><button class="file-card-preview" data-action="file" data-file="${id}" aria-label="预览 ${f.name}">${id==='avatars'?avatarIds.slice(0,6).map(a=>avatar(a)).join(''):id==='icons'?['node','leader','worker','task','review','workspace'].map(i=>icon(i)).join(''):icon('result','large-icon')}</button>${fileRow(id)}</article>`).join('')}</div></div>`;
}
function renderLeader(){
  const l=leaders[view.leader];
  $('detail-header').innerHTML=topBar('长期协作')+`<div class="task-heading"><div class="task-heading-title">${avatar(l.avatar,'nav-avatar')}<h2>${l.name}</h2></div><div class="task-context">同一段长期对话，保留你的目标与上下文。</div></div>`;
  const related=tasks.filter(t=>t.leader===view.leader).slice(0,2);
  $('detail-scroll').innerHTML=`<div class="leader-chat"><div class="leader-intro">${avatar(l.avatar)}<div><h3>接着上次，一起往前走。</h3><p>${l.description}</p></div></div>${message('worker','你可以直接说想推进的事。我会和你理清目标，再把具体工作交给合适的伙伴。',null,true)}${related.map(t=>`<button class="inline-task" data-action="task" data-task="${t.id}">${icon('task')}<span>${escapeHtml(t.title)}</span>${badge(t.state)}</button>`).join('')}<div style="height:24px"></div>${chats[view.leader].map(([who,text])=>message(who,text,null,true)).join('')}${composer('leader')}</div>`;
  $('detail-footer').innerHTML='';
}
function render(){renderNav();renderList();view.mode==='files'?renderFiles():view.mode==='leader'?renderLeader():renderTask();$('detail-scroll').scrollTop=0;}

function openModal(title,body){$('dialog-content').innerHTML=`<div class="modal-head"><h2 id="dialog-title">${title}</h2><button class="icon-button" data-action="close-dialog" aria-label="关闭">${icon('x')}</button></div><div class="modal-body">${body}</div>`;if(!$('dialog').open)$('dialog').showModal();}
function newGoal(){openModal('想推进什么？',`<p>先说清目标，把执行安排交给 Leader。</p><form id="new-goal-form"><label class="form-field"><span>目标</span><textarea name="goal" required maxlength="600" placeholder="例如：把新头像接入 Agent 设置，并检查小尺寸显示效果。"></textarea></label><label class="form-field"><span>交给</span><select name="leader">${Object.entries(leaders).map(([key,l])=>`<option value="${key}" ${key===view.leader?'selected':''}>${l.name}</option>`).join('')}</select></label><div class="modal-actions"><button class="button ghost" type="button" data-action="close-dialog">取消</button><button class="button primary" type="submit">开始推进</button></div></form>`);}
function resources(){openModal('设备与模型',`<p>设备是执行资源。它们暂时离线时，已同步的结果仍然可以查看。</p>${Object.entries(nodes).map(([name,online])=>`<div class="resource-row">${icon('node')}<div><b>${name}</b><small>${name==='mini1'?'本机设备':'远端设备'} · ${online?'在线，模型连接可用':'暂时离线，保留已同步内容'}</small></div><button class="resource-toggle" role="switch" aria-label="${name} 连接状态（样稿）" aria-checked="${online}" data-action="toggle-node" data-node="${name}"></button></div>`).join('')}<p class="muted" style="margin-top:20px;font-size:12px">这里仅切换样稿的显示状态，不会连接或断开真实设备。</p>`);}
function previewFile(id){const f=files[id];if(!f)return;let body=id==='avatars'?`<div class="modal-avatars">${avatarIds.map(i=>avatar(i)).join('')}</div>`:id==='icons'?`<div class="modal-icons">${iconIds.map(i=>`<div class="modal-icon">${icon(i)}</div>`).join('')}</div>`:`<div class="guide-text"><h3>一套身份，各有职责。</h3><p>品牌代表 zork；Leader 和 Worker 表示角色；头像表示个体；状态说明当前发生的事。</p><h3>从目标走到交付。</h3><p>任务保存原始目标、讨论与版本化结果。提交结果后等待用户验收，验收当前版本后才进入已完成。</p><h3>资源可维护。</h3><p>功能图标、头像和简单插画保留 SVG，语义颜色与几何尺寸用 token 管理。</p></div>`;openModal(f.name,body+`<div class="modal-actions"><a class="button primary" href="${f.href}" download>${icon('download')}下载文件</a></div>`);}

document.addEventListener('click',event=>{
  const el=event.target.closest('[data-action]');if(!el)return;
  const action=el.dataset.action;
  if(action==='mode')selectMode(el.dataset.mode);
  else if(action==='leader')selectMode('leader',el.dataset.leader);
  else if(action==='task')selectTask(el.dataset.task);
  else if(action==='tab'){view.tab=el.dataset.tab;renderTask();$('detail-scroll').scrollTop=0;}
  else if(action==='new-goal')newGoal();
  else if(action==='resources')resources();
  else if(action==='file')previewFile(el.dataset.file);
  else if(action==='close-dialog')$('dialog').close();
  else if(action==='toggle-nav')showNav();
  else if(action==='show-list'){document.body.classList.add('mobile-list');}
  else if(action==='toggle-node'){nodes[el.dataset.node]=!nodes[el.dataset.node];resources();render();}
  else if(action==='accept'){const t=currentTask();if(t?.state==='review'){t.state='completed';t.excerpt='已接受当前版本，结果继续保留';t.age='已交付';render();flash('已接受版本 '+t.version);}}
  else if(action==='revise'){view.tab='discussion';renderTask();requestAnimationFrame(()=>{const input=document.querySelector('#discussion-form textarea');input?.focus();input?.scrollIntoView({block:'nearest'});});}
  else if(action==='reopen'){const t=currentTask();if(t){t.state='open';t.excerpt='任务已重新打开，等待后续安排';view.tab='discussion';render();flash('任务已重新打开');}}
  else if(action==='reset'){tasks=structuredClone(initialTasks);view={mode:'attention',task:'brand',tab:'delivery',leader:'product',query:''};nodes={mini1:true,mini2:true};chats={product:[],engineering:[],research:[]};$('task-search').value='';closeNav();document.body.classList.remove('mobile-list');render();flash('样稿已重置');}
});
document.addEventListener('submit',event=>{
  const form=event.target;event.preventDefault();const data=new FormData(form);
  if(form.id==='new-goal-form'){
    const goal=String(data.get('goal')||'').trim(),leader=String(data.get('leader')||'product');if(!goal||!leaders[leader])return;
    const l=leaders[leader],id='new-'+Date.now();tasks.unshift({id,title:goal.split('\n')[0].slice(0,32),goal,state:'open',leader,worker:l.worker,workerName:l.workerName,node:l.node,excerpt:'目标已记录，等待 Leader 安排',age:'刚刚',version:0,files:[],discussion:[['user',goal],['worker','目标已记录。接下来会先确认执行范围与验收要求。']]});$('dialog').close();view.mode='all';view.query='';$('task-search').value='';selectTask(id);flash('新目标已加入任务');
  }else if(form.id==='discussion-form'){
    const text=String(data.get('message')||'').trim(),t=currentTask();if(!text||!t)return;t.discussion.push(['user',text]);if(['review','completed'].includes(t.state)){t.state='open';t.excerpt='有新的修改要求，等待继续推进';}view.tab='discussion';render();$('detail-scroll').scrollTop=$('detail-scroll').scrollHeight;
  }else if(form.id==='leader-form'){
    const text=String(data.get('message')||'').trim();if(!text)return;chats[view.leader].push(['user',text],['worker','收到，我们可以围绕这个目标继续细化。准备好进入执行时，用“新目标”把它保存为独立任务。']);renderLeader();$('detail-scroll').scrollTop=$('detail-scroll').scrollHeight;
  }else if(form.id==='decision-form'){
    const t=currentTask();if(!t)return;t.discussion.push(['user',data.get('policy')==='safe'?'默认仅自动重试可安全重复的操作。':'默认先暂停，由我确认后继续。'],['worker','已记录这个决定，接下来按这一策略继续推进。']);t.state='working';t.excerpt='策略已确认，继续处理恢复体验';view.tab='discussion';render();flash('决定已记录，任务继续推进');
  }
});
$('task-search').addEventListener('input',e=>{view.query=e.target.value;renderList();});
$('mobile-scrim').addEventListener('click',closeNav);
$('dialog').addEventListener('click',e=>{if(e.target===$('dialog')){const r=$('dialog').getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)$('dialog').close();}});
document.addEventListener('keydown',e=>{
  if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='k'){e.preventDefault();if(view.mode==='files')selectMode('all');document.body.classList.add('mobile-list');$('task-search').focus();}
  if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='n'){e.preventDefault();newGoal();}
  if(e.key==='Escape')closeNav();
});
render();
