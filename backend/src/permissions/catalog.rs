//! Каталог гранулярных прав. Единый источник истины для сервера и UI.
//! При добавлении права оно автоматически синхронизируется в таблицу permissions.

pub const PERMISSIONS: &[(&str, &str, &str)] = &[
    ("minecraft.view", "minecraft", "Просмотр сборок Minecraft"),
    ("minecraft.build.create", "minecraft", "Создание сборок"),
    ("minecraft.build.edit", "minecraft", "Редактирование сборок"),
    ("minecraft.build.publish", "minecraft", "Публикация релизов"),
    ("minecraft.build.delete", "minecraft", "Удаление сборок"),
    (
        "minecraft.artifacts.upload",
        "minecraft",
        "Загрузка файлов сборок",
    ),
    (
        "launcher.manage",
        "launcher",
        "Управление токенами лаунчера",
    ),
    ("cloud.view", "cloud", "Доступ к облачному хранилищу"),
    ("cloud.file.upload", "cloud", "Загрузка файлов"),
    ("cloud.file.download", "cloud", "Скачивание файлов"),
    ("cloud.file.delete", "cloud", "Удаление файлов"),
    ("cloud.file.share", "cloud", "Общий доступ к файлам"),
    ("cloud.folder.create", "cloud", "Создание папок"),
    ("cloud.storage.manage", "cloud", "Управление хранилищами"),
    ("system.view", "system", "Просмотр состояния системы"),
    ("system.users.manage", "system", "Управление пользователями"),
    (
        "system.storage.manage",
        "system",
        "Управление дисками и хранилищами",
    ),
    ("system.settings.manage", "system", "Управление настройками"),
    ("system.monitoring.view", "system", "Просмотр мониторинга"),
    (
        "system.jobs.manage",
        "system",
        "Управление фоновыми задачами",
    ),
    ("ai.access", "ai", "Доступ к AI Workspace"),
    ("ai.chat.use", "ai", "Использование AI Chat"),
    ("ai.agent.use", "ai", "Запуск AI Agent задач"),
    ("ai.workspace.read", "ai", "Чтение рабочего пространства AI"),
    (
        "ai.workspace.write",
        "ai",
        "Изменение рабочего пространства AI",
    ),
    ("ai.skills.use", "ai", "Использование Skills"),
    ("ai.tools.use", "ai", "Использование инструментов Agent"),
    ("ai.tasks.cancel", "ai", "Отмена AI задач"),
    ("ai.actions.approve", "ai", "Подтверждение опасных действий"),
    ("ai.admin.manage", "ai", "Администрирование AI"),
];

pub fn is_valid(key: &str) -> bool {
    PERMISSIONS.iter().any(|(k, _, _)| *k == key)
}

pub fn all_keys() -> Vec<String> {
    PERMISSIONS
        .iter()
        .map(|(k, _, _)| (*k).to_owned())
        .collect()
}

pub fn module_of(key: &str) -> Option<&'static str> {
    PERMISSIONS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, m, _)| *m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_permissions_are_registered() {
        for key in [
            "ai.access",
            "ai.chat.use",
            "ai.agent.use",
            "ai.workspace.read",
            "ai.workspace.write",
            "ai.skills.use",
            "ai.tools.use",
            "ai.tasks.cancel",
            "ai.actions.approve",
            "ai.admin.manage",
        ] {
            assert!(is_valid(key), "право {key} отсутствует в каталоге");
            assert_eq!(module_of(key), Some("ai"));
        }
    }
}
