# PSScriptAnalyzer: базовый набор правил для всех .ps1 репозитория (K3).
#
# Набор намеренно узкий: это правила корректности и безопасности, которые
# обязаны быть чистыми. Стилистические правила (PSAvoidUsingWriteHost,
# PSUseConsistentIndentation, PSAvoidUsingCmdletAliases) не включены: скрипты
# развёртывания и приёмки — интерактивные консольные утилиты, а форматирование
# историческое. Отдельно исключены:
#   * PSAvoidUsingEmptyCatchBlock — пустой catch используется осознанно как
#     «проверить, не занят ли порт/не отвечает ли health» без шума в журнале;
#   * PSAvoidUsingPlainTextForPassword — параметры вида -AdminPassword и
#     -TestPasswordEnv принимают строку/имя переменной для автоматизации;
#   * PSAvoidUsingWMICmdlet — Get-WmiObject оставлен как fallback удаления
#     VSS-снимка в Windows PowerShell 5.1, где CIM-метод недоступен.
#
# Запуск локально:
#   Invoke-ScriptAnalyzer -Path deployment,tests -Recurse `
#     -Settings ./PSScriptAnalyzerSettings.psd1
#
# В CI шаг падает при любом найденном нарушении.

@{
    Severity     = @('Error', 'Warning')
    IncludeRules = @(
        'PSAvoidAssignmentToAutomaticVariable',
        'PSAvoidUsingBrokenHashAlgorithms',
        'PSAvoidUsingConvertToSecureStringWithPlainText',
        'PSAvoidUsingInvokeExpression',
        'PSPossibleIncorrectComparisonWithNull',
        'PSUseApprovedVerbs',
        'PSUseBOMForUnicodeEncodedFile',
        'PSUseCmdletCorrectly',
        'PSUseDeclaredVarsMoreThanAssignments'
    )
    ExcludeRules = @(
        'PSAvoidUsingEmptyCatchBlock',
        'PSAvoidUsingPlainTextForPassword',
        'PSAvoidUsingWMICmdlet'
    )
}
