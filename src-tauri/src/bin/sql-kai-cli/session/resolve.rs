//! Резолв алиаса в профиль: точный id, иначе имя, иначе группа.

use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

/// Все профили, подходящие под alias: точный id, иначе имя, иначе группа
/// (имя/группа — без учёта регистра). Мульти-матч для history/doctor — та же
/// логика, что в [`resolve_profile`], но без ошибок неоднозначности.
pub fn filter_profiles(all: &[Profile], alias: &str) -> Vec<Profile> {
    if let Some(p) = all.iter().find(|p| p.id == alias) {
        return vec![p.clone()];
    }
    let by_name: Vec<Profile> = all
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(alias))
        .cloned()
        .collect();
    if !by_name.is_empty() {
        return by_name;
    }
    all.iter()
        .filter(|p| {
            p.group
                .as_deref()
                .is_some_and(|g| g.eq_ignore_ascii_case(alias))
        })
        .cloned()
        .collect()
}

/// Алиас = id, имя или группа профиля (имя/группа — без учёта регистра).
pub fn resolve_profile(alias: &str) -> Result<Profile, AppError> {
    let all = store::load_profiles()?;
    if let Some(p) = all.iter().find(|p| p.id == alias) {
        return Ok(p.clone());
    }
    let by_name: Vec<&Profile> = all
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(alias))
        .collect();
    if by_name.len() == 1 {
        return Ok(by_name[0].clone());
    }
    if by_name.len() > 1 {
        return Err(AppError::Msg(format!(
            "несколько профилей с именем '{alias}' — укажи id (sql-kai profiles list)"
        )));
    }
    let by_group: Vec<&Profile> = all
        .iter()
        .filter(|p| {
            p.group
                .as_deref()
                .map(|g| g.eq_ignore_ascii_case(alias))
                .unwrap_or(false)
        })
        .collect();
    match by_group.len() {
        1 => Ok(by_group[0].clone()),
        0 => {
            let known = all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(AppError::Msg(format!(
                "профиль '{alias}' не найден (есть: {known}); для нового прод-хоста: sql-kai discover {alias}"
            )))
        }
        _ => {
            let names = by_group.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(AppError::Msg(format!(
                "в группе '{alias}' несколько профилей — уточни: {names}"
            )))
        }
    }
}
