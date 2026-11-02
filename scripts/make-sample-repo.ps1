<#
.SYNOPSIS
  Creates a sample project with a long history for trying the timeline by hand.

.DESCRIPTION
  Wipes and recreates %TEMP%\savestates-sample:
  - 1,500 saves on the current timeline (main), with realistic PT-BR names, some longer than
    40 characters, spread over about a year and a half and ending today;
  - a `feature` timeline joined back into main by a merge commit (two parents, as
    `git merge --no-ff` makes), and kept, so the app shows its "other timelines" note;
  - 3 unsaved changes (a changed file, a new file and a deleted file).

  The history is written with one `git fast-import` run, so the whole script takes seconds.
#>

$ErrorActionPreference = 'Stop'

$Repo = Join-Path ([System.IO.Path]::GetTempPath()) 'savestates-sample'
$SaveCount = 1500
# The first-parent position of the merge commit (counted from the first save).
$MergeAt = 1460
# Where the feature timeline branches off main.
$ForkAt = 1440
$Zone = '-0300'
$Author = 'Ana Souza <ana.souza@example.com>'

$Names = @(
    'Arruma o menu no celular',
    'Página de contato com formulário que envia e-mail de verdade',
    'Modo escuro',
    'Galeria de fotos',
    'Corrige o botão de enviar',
    'Troca as cores do cabeçalho para combinar com o logotipo novo',
    'Texto da página inicial',
    'Ajusta o espaçamento dos cards',
    'Adiciona rodapé com redes sociais',
    'Melhora o carregamento das imagens grandes na galeria de fotos',
    'Arruma acentos nos títulos',
    'Nova seção de depoimentos',
    'Deixa o site mais rápido no celular',
    'Corrige link quebrado',
    'Tela de login simples',
    'Refaz o layout da página Sobre para ficar igual ao desenho aprovado',
    'Ícones novos',
    'Ajustes finos',
    'Lista de produtos com filtro por categoria e por faixa de preço',
    'Animação no botão principal',
    'Remove código que não era usado',
    'Corrige erro ao abrir no Safari',
    'Fonte nova nos títulos',
    'Antes de mexer no carrinho',
    'Carrinho de compras funcionando, falta só calcular o frete',
    'Mapa na página de contato',
    'Texto revisado',
    'Versão para imprimir',
    'Botão de voltar ao topo'
)
$FeatureNames = @(
    'Começa a loja',
    'Página de produto',
    'Carrinho na loja',
    'Ajusta os preços',
    'Loja pronta para juntar'
)
$Files = @(
    'index.html',
    'css/estilo.css',
    'js/app.js',
    'README.md',
    'paginas/contato.html',
    'paginas/galeria.html',
    'paginas/sobre.html'
)

$Utf8 = New-Object System.Text.UTF8Encoding($false)
$Stream = New-Object System.Text.StringBuilder

function Add-Line([string]$Text) {
    [void]$Stream.Append($Text).Append("`n")
}

# A `data` block: the exact UTF-8 byte count, then the bytes.
function Add-Data([string]$Text) {
    Add-Line "data $($Utf8.GetByteCount($Text))"
    [void]$Stream.Append($Text).Append("`n")
}

function Add-File([string]$Path, [string]$Content) {
    Add-Line "M 100644 inline $Path"
    Add-Data $Content
}

function Add-Commit([string]$Ref, [int]$Mark, [long]$Time, [string]$Message) {
    Add-Line "commit $Ref"
    Add-Line "mark :$Mark"
    Add-Line "author $Author $Time $Zone"
    Add-Line "committer $Author $Time $Zone"
    Add-Data "$Message`n"
}

# Times, oldest first: the newest save is 47 minutes ago, and the gaps before it vary between
# 10 minutes and about 20 hours (a fixed pseudo-random sequence, so every run looks the same).
$Now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$Times = New-Object 'long[]' ($SaveCount + 1)
$Times[$SaveCount] = $Now - 47 * 60
[long]$Seed = 12345
for ($i = $SaveCount - 1; $i -ge 1; $i--) {
    $Seed = ($Seed * 1103515245 + 12345) % 2147483648
    [long]$Gap = 600 + ($Seed % 72000)
    $Times[$i] = $Times[$i + 1] - $Gap
}

# Marks: save i of main is mark i; the feature commits come after them.
$FeatureMark = $SaveCount + 1

for ($i = 1; $i -le $SaveCount; $i++) {
    if ($i -eq $ForkAt + 1) {
        # The feature timeline: five commits that start from save $ForkAt.
        $Parent = $ForkAt
        for ($f = 0; $f -lt $FeatureNames.Count; $f++) {
            $Mark = $FeatureMark + $f
            $Time = $Times[$ForkAt] + 300 * ($f + 1)
            Add-Commit 'refs/heads/feature' $Mark $Time $FeatureNames[$f]
            Add-Line "from :$Parent"
            Add-File 'paginas/loja.html' "<h1>Loja</h1>`n<p>passo $($f + 1)</p>`n"
            Add-Line ''
            $Parent = $Mark
        }
    }

    if ($i -eq 1) {
        Add-Commit 'refs/heads/main' 1 $Times[1] 'Começo do projeto'
        foreach ($Path in $Files) {
            Add-File $Path "versão 1 de $Path`n"
        }
    } elseif ($i -eq $MergeAt) {
        Add-Commit 'refs/heads/main' $i $Times[$i] 'Junta a loja nova'
        Add-Line "from :$($i - 1)"
        Add-Line "merge :$($FeatureMark + $FeatureNames.Count - 1)"
        Add-File 'paginas/loja.html' "<h1>Loja</h1>`n<p>passo $($FeatureNames.Count)</p>`n"
    } else {
        Add-Commit 'refs/heads/main' $i $Times[$i] $Names[$i % $Names.Count]
        Add-Line "from :$($i - 1)"
        # One to three files per save.
        $Touched = 1 + ($i % 3)
        for ($k = 0; $k -lt $Touched; $k++) {
            $Path = $Files[($i + $k * 3) % $Files.Count]
            Add-File $Path "versão $i de $Path`n"
        }
    }
    Add-Line ''
}
Add-Line 'done'

if (Test-Path $Repo) {
    Remove-Item -Recurse -Force $Repo
}
New-Item -ItemType Directory -Path $Repo | Out-Null

$StreamFile = Join-Path ([System.IO.Path]::GetTempPath()) 'savestates-sample.fi'
[System.IO.File]::WriteAllText($StreamFile, $Stream.ToString(), $Utf8)

try {
    git -C $Repo init -q -b main
    if ($LASTEXITCODE -ne 0) { throw 'git init failed' }

    # Windows PowerShell would re-encode a pipe, so git reads the stream from the file.
    $Import = Start-Process -FilePath 'git' -ArgumentList @('-C', "`"$Repo`"", 'fast-import', '--quiet', '--done') `
        -RedirectStandardInput $StreamFile -NoNewWindow -Wait -PassThru
    if ($Import.ExitCode -ne 0) { throw "git fast-import failed ($($Import.ExitCode))" }

    git -C $Repo reset -q --hard main
    if ($LASTEXITCODE -ne 0) { throw 'git reset failed' }
} finally {
    Remove-Item -Force $StreamFile -ErrorAction SilentlyContinue
}

# Three unsaved changes: one changed, one new, one deleted.
Add-Content -Path (Join-Path $Repo 'css/estilo.css') -Value '.menu { gap: 12px; }'
Set-Content -Path (Join-Path $Repo 'rascunho.txt') -Value 'ideias para a próxima versão'
Remove-Item -Path (Join-Path $Repo 'paginas/galeria.html')

$Saves = git -C $Repo rev-list --first-parent --count main
$Unsaved = (git -C $Repo status --porcelain | Measure-Object).Count
Write-Host "Created $Repo with $Saves saves on main, a merged 'feature' timeline and $Unsaved unsaved changes."
