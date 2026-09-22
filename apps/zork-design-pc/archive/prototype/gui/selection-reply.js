/* Selected passages become draft comments, then travel with one message. */
'use strict';
let selectedReply=null;
const pendingComments={};
function closeSelectionReply(){selectedReply=null;$('selection-reply').hidden=true;$('selection-reply').innerHTML='';}
function getPendingComments(id){return (pendingComments[id]||[]).map(comment=>({...comment}));}
function clearPendingComments(id){delete pendingComments[id];}
function renderPendingComments(){
 const el=$('pending-comments');if(!el)return;
 const comments=getPendingComments(current);el.hidden=!comments.length;
 el.innerHTML=comments.length?`<div class="pending-comments-head"><b>${comments.length} 条评论</b><span>随消息一起发送</span></div><ol>${comments.map((comment,index)=>`<li data-comment-id="${comment.id}"><span class="comment-number">${index+1}</span><div class="pending-comment-copy"><blockquote>${esc(nameOf(comment.author))}：${esc(comment.quote)}</blockquote><p>${esc(comment.text)}</p></div><div class="pending-comment-actions"><button data-action="edit-comment" data-comment="${comment.id}">编辑</button><button class="icon-button" data-action="remove-comment" data-comment="${comment.id}" aria-label="移除第 ${index+1} 条评论">${icon('x')}</button></div></li>`).join('')}</ol>`:'';
}
function renderMessageComments(comments){return `<div class="message-comments">${comments.map((comment,index)=>`<div class="sent-comment"><div class="sent-comment-label">评论 ${index+1}</div><blockquote>${esc(nameOf(comment.author))}：${esc(comment.quote)}</blockquote><p class="message-text">${esc(comment.text)}</p></div>`).join('')}</div>`;}
function openCommentEditor(context,rect,text=''){
 selectedReply={...context};const el=$('selection-reply');
 el.innerHTML=`<form id="selection-reply-form"><div class="selection-reply-head"><span>${context.commentId?'编辑评论':'评论这段内容'}</span><button class="icon-button" type="button" data-action="close-selection-reply" aria-label="关闭评论输入框">${icon('x')}</button></div><blockquote>${esc(context.text)}</blockquote><textarea name="reply-text" aria-label="针对选中文字的评论" placeholder="针对这段内容说点什么…" maxlength="4000"></textarea><div class="selection-reply-footer"><span>Enter ${context.commentId?'保存':'添加'} · 尚未发送</span><button class="comment-add-button" type="submit" ${text.trim()?'':'disabled'}>${context.commentId?'保存评论':'添加评论'}</button></div></form>`;
 el.querySelector('textarea').value=text;el.hidden=false;
 const width=Math.min(330,window.innerWidth-24);el.style.width=width+'px';el.style.left=Math.max(12,Math.min(rect.left,window.innerWidth-width-12))+'px';
 const height=el.getBoundingClientRect().height,top=rect.bottom+8+height<=window.innerHeight-12?rect.bottom+8:rect.top-height-8;
 el.style.top=Math.max(12,Math.min(top,window.innerHeight-height-12))+'px';el.querySelector('textarea').focus();
}
function showSelectionReply(){
 const selection=window.getSelection?.();if(!selection||selection.isCollapsed||!selection.rangeCount)return;
 const parent=node=>node?.nodeType===3?node.parentElement:node;
 const start=parent(selection.anchorNode)?.closest?.('.message-text'),end=parent(selection.focusNode)?.closest?.('.message-text');
 if(!start||start!==end||!start.closest('#messages'))return;
 const article=start.closest('.chat-message'),msg=chat().messages.find(m=>m.id===article?.id),text=selection.toString().trim();if(!msg||!text)return;
 openCommentEditor({chatId:current,messageId:msg.id,author:msg.who,text},selection.getRangeAt(0).getBoundingClientRect());
}
document.addEventListener('pointerup',e=>{if(!e.target.closest('#selection-reply,button,a,input,textarea'))requestAnimationFrame(showSelectionReply);});
document.addEventListener('keyup',e=>{if(e.shiftKey&&['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(e.key))showSelectionReply();});
document.addEventListener('pointerdown',e=>{if(selectedReply&&!e.target.closest('#selection-reply'))closeSelectionReply();});
document.addEventListener('click',e=>{
 const b=e.target.closest('[data-action]');if(!b)return;
 if(b.dataset.action==='close-selection-reply')closeSelectionReply();
 else if(b.dataset.action==='remove-comment'){pendingComments[current]=getPendingComments(current).filter(comment=>comment.id!==b.dataset.comment);renderPendingComments();}
 else if(b.dataset.action==='edit-comment'){
  const comment=getPendingComments(current).find(comment=>comment.id===b.dataset.comment);if(!comment)return;
  openCommentEditor({chatId:current,messageId:comment.messageId,author:comment.author,text:comment.quote,commentId:comment.id},b.getBoundingClientRect(),comment.text);
 }
});
document.addEventListener('input',e=>{if(e.target.matches('#selection-reply textarea'))$('selection-reply').querySelector('[type="submit"]').disabled=!e.target.value.trim();});
document.addEventListener('keydown',e=>{
 if(e.key==='Escape'&&selectedReply){e.preventDefault();closeSelectionReply();}
 if(e.target.matches('#selection-reply textarea')&&e.key==='Enter'&&!e.shiftKey&&!e.isComposing){e.preventDefault();$('selection-reply-form').requestSubmit();}
});
document.addEventListener('submit',e=>{
 if(e.target.id!=='selection-reply-form')return;e.preventDefault();const text=e.target.querySelector('textarea').value.trim();if(!text||!selectedReply||selectedReply.chatId!==current)return;
 const comments=pendingComments[current]||(pendingComments[current]=[]);
 if(selectedReply.commentId){const comment=comments.find(comment=>comment.id===selectedReply.commentId);if(comment)comment.text=text;}
 else comments.push({id:'comment-'+(++sequence),messageId:selectedReply.messageId,author:selectedReply.author,quote:selectedReply.text,text});
 closeSelectionReply();renderPendingComments();
});
window.addEventListener('resize',closeSelectionReply);
$('message-scroll').addEventListener('scroll',closeSelectionReply,{passive:true});
renderPendingComments();
