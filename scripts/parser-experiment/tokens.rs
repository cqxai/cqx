// Direct port of @lezer/cpp 1.1.6 src/tokens.js (MIT).
use rezel_common::ParseError;
use rezel_lr::{ExternalTokenizer, InputStream, Stack, TokenizerFlags};
use crate::terms::{RawString, templateArgsEndFallback, MacroName};
fn next(i: &InputStream) -> u32 { i.next().map(|c|c.as_u32()).unwrap_or(u32::MAX) }
fn raw(i: &mut InputStream, _: &Stack) -> Result<(),ParseError> {
    match next(i) {76|85=>{i.advance(1);},117=>{i.advance(1);if next(i)==56 {i.advance(1);}},_=>{}}
    if next(i)!=82 {return Ok(());} i.advance(1);
    if next(i)!=34 {return Ok(());} i.advance(1);
    let mut marker=Vec::new();
    while next(i)!=40 {
        let n=next(i);if n==32||n<=13||n==41||n==u32::MAX{return Ok(());}
        marker.push(n);i.advance(1);
    }
    i.advance(1);
    loop {
        if next(i)==u32::MAX {return i.accept_token(RawString);}
        if next(i)==41 && marker.iter().enumerate().all(|(n,c)|i.peek(n as isize+1).map(|v|v.as_u32())==Some(*c))
          && i.peek(marker.len() as isize+1).map(|v|v.as_u32())==Some(34) {
            i.advance(marker.len()+2);return i.accept_token(RawString);
        }
        i.advance(1);
    }
}
fn fallback_token(i:&mut InputStream,_:&Stack)->Result<(),ParseError>{
    if next(i)==62 {if i.peek(1).map(|c|c.as_u32())==Some(62){i.advance(1);i.accept_token(templateArgsEndFallback)?;}}
    else {
        let mut letter=false;let mut len=0;
        loop {let n=next(i);if (65..=90).contains(&n){letter=true;}else if (97..=122).contains(&n){return Ok(());}else if n!=95&&!(48..=57).contains(&n){break;}i.advance(1);len+=1;}
        if letter&&len>1{i.accept_token(MacroName)?;}
    }
    Ok(())
}
#[allow(non_upper_case_globals)] // Names match the upstream grammar bindings.
pub static rawString:ExternalTokenizer=ExternalTokenizer::new(raw,TokenizerFlags{contextual:false,fallback:false,extend:false});
#[allow(non_upper_case_globals)] // Names match the upstream grammar bindings.
pub static fallback:ExternalTokenizer=ExternalTokenizer::new(fallback_token,TokenizerFlags{contextual:false,fallback:false,extend:true});
