# Порты, HTTP.sys, Hyper-V/WSL и firewall

Дата: 20.11.2026. Стенд: Windows 11 Pro 10.0.26300, PowerShell 5.1,
release-сборка NEXUS.

## 1. Фактическая картина на стенде

| Проверка | Результат |
|---|---|
| Исключённые диапазоны TCP IPv4 | `13508–13508` (managed) |
| Исключённые диапазоны TCP IPv6 | `13508–13508` (managed) |
| HTTP.sys | `Running`; активные URL: `HTTP://127.0.0.1:13508/.../MCP/` (NVIDIA nvcontainer) |
| IIS / W3SVC | не установлена |
| Слушатели 80/443/8080 | отсутствуют |
| Bind 80 / 443 / 8080 | `OK` |
| URL ACL HTTP.sys на 80/443 | есть (`http://+:80/Temporary_Listen_Addresses/`, `https://+:443/...`), но активных слушателей нет |

Вывод: на этом стенде 80/443/8080 свободны. Наличие URL ACL само по себе не
мешает обычному TCP-bind; оно важно для `HttpListener` и для диагностики
HTTP.sys. Реальный конфликт возникает, когда порт слушает `System` (PID 4,
HTTP.sys/IIS) или порт входит в исключённый диапазон Hyper-V/WSL.

## 2. Проверка исключённого диапазона

Временно добавлен диапазон `18091–18092`:

```powershell
netsh interface ipv4 add excludedportrange protocol=tcp startport=18091 numberofports=2 store=persistent
powershell -File deployment\check-ports.ps1 -Ports 18091,18092
# Порт 18091: excluded ... Порт 18092: excluded ... exit 1
netsh interface ipv4 delete excludedportrange protocol=tcp startport=18091 numberofports=2
```

`check-ports.ps1` корректно распознал managed-диапазон (строка со `*`) и
вернул код 1. После удаления диапазона список снова содержит только
`13508–13508`.

Примечание: на этом стенде ручной `TcpListener` внутри только что добавленного
«исключённого» диапазона всё равно успел выполнить bind. Поэтому решение
принимается по `netsh ... show excludedportrange` (источник истины для
Hyper-V/WSL), а bind-тест используется как дополнительная диагностика
(`AccessDenied`/`AddressAlreadyInUse`).

## 3. Firewall

Проверено на тестовых портах, чтобы не открывать реальные 80/443 без Caddy:

```powershell
powershell -File deployment\firewall-windows.ps1 -HttpPort 18080 -HttpsPort 18443 -DisplayNamePrefix 'NEXUS TEST'
# NEXUS TEST - HTTP 18080 / NEXUS TEST - HTTPS 18443, Enabled=True, Inbound, Allow
powershell -File deployment\firewall-windows.ps1 -HttpPort 18080 -HttpsPort 18443 -DisplayNamePrefix 'NEXUS TEST'   # идемпотентно
powershell -File deployment\firewall-windows.ps1 -HttpPort 18080 -HttpsPort 18443 -DisplayNamePrefix 'NEXUS TEST' -Remove
```

Backend-правило с ограничением источника:

```powershell
powershell -File deployment\firewall-windows.ps1 -SkipHttp -SkipHttps -ExposeBackend `
  -BackendPort 18081 -ProxyAddress 10.0.0.10 -DisplayNamePrefix 'NEXUS TEST'
# NEXUS TEST - backend 18081, Inbound Allow
```

После `-Remove` правил с префиксом не осталось (Count=0).

## 4. Интеграция с установщиком

`install-windows.ps1 -SkipBuild -SkipAdmin -SkipService` на временном каталоге:

* прочитал `NEXUS_PORT` из `.env` и проверил 8080 с `-RequireFree`;
* проверил 80/443 без bind-теста (занятость Caddy допустима, критичны только
  исключённые диапазоны);
* продолжил миграции и `nexus check` (exit 0).

С `-ConfigureFirewall` установщик вызывает `firewall-windows.ps1` после старта
службы. По умолчанию firewall не меняется.

## 5. Итог

* `deployment/check-ports.ps1` — единая проверка слушателей, HTTP.sys,
  исключённых диапазонов и bind;
* `deployment/firewall-windows.ps1` — идемпотентные правила 80/443, опционально
  8080 только с адреса прокси;
* установщик автоматически проверяет 8080 и 80/443, опционально настраивает
  firewall;
* на стенде 80/443/8080 пригодны, Hyper-V/WSL диапазоны отсутствуют
  (кроме managed `13508` для NVIDIA HTTP.sys).
