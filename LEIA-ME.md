# SaveStates: leia-me

O SaveStates guarda versões do seu projeto enquanto você trabalha, e deixa você voltar a qualquer uma delas quando quiser. É como o "salvar" de um jogo: você faz um **save** quando está tudo bem, e se algo der errado depois, é só olhar para trás e trazer de volta o que precisa.

Este programa é feito para o computador pessoal de quem recebeu o instalador de quem o criou. Ele funciona no Windows 10 e no Windows 11 (64 bits).

## O que você precisa ter antes

1. **Git for Windows** (obrigatório, e é gratuito). O SaveStates usa o Git por baixo dos panos para guardar os seus saves.
   - Baixe em <https://git-scm.com/download/win> e instale deixando todas as opções como já vêm (é só clicar em **Next** até o fim).
   - Se o Git não estiver instalado, o SaveStates abre mesmo assim e mostra um aviso com um botão para baixá-lo.
2. **Claude Code** (opcional). Só serve se você quiser que a IA sugira o nome dos seus saves. Ele precisa estar instalado e com o login feito.
   - Sem o Claude Code, o SaveStates simplesmente pergunta o nome de cada save. Nada mais muda.
   - Quando você liga a IA, as alterações de cada save são enviadas à Anthropic pela sua conta do Claude Code. O programa avisa isso na primeira vez.
3. **Uma placa de vídeo com o driver em dia.** O SaveStates desenha a tela com Vulkan, que qualquer placa de uns 2016 para cá entende, desde que o driver esteja atualizado (veja "Se aparecer um erro de gráficos", mais abaixo).

## Instalando

1. Baixe o arquivo **SaveStates-Setup-0.1.0.exe** que você recebeu e dê dois cliques nele.
2. O Windows vai mostrar um aviso de segurança. Isso é esperado: veja a próxima seção.
3. Clique em **Avançar** e depois em **Instalar** (se o instalador perguntar o idioma, escolha o seu). Não pede senha de administrador: o programa é instalado só para você, em `%LOCALAPPDATA%\Programs\SaveStates`.
4. Ao terminar, deixe marcada a opção para abrir o SaveStates. Depois, ele fica no **Menu Iniciar**, em "SaveStates". Na instalação você também pode pedir um atalho na área de trabalho.

### O aviso do Windows ("O Windows protegeu o computador")

O instalador não tem assinatura digital (assinar custa caro, e este programa é só para amigos). Por isso o Windows desconfia dele e mostra uma tela azul:

1. Quando aparecer **"O Windows protegeu o computador"**, não feche: clique em **Mais informações**.
2. Vai aparecer um novo botão. Clique em **Executar assim mesmo**.
3. O instalador abre e você segue normalmente.

Faça isso só com o instalador que veio direto de quem criou o SaveStates. Se um arquivo com esse nome chegar por outro caminho, não instale.

## Primeiro uso

1. Clique em **Abrir projeto** e escolha a pasta do seu projeto.
2. Se a pasta ainda não tem saves, o SaveStates pergunta se pode ativar os saves nela. Clique em **Ativar saves nesta pasta**.
3. Na primeira vez que você salvar, ele pede o seu nome e um e-mail. Qualquer e-mail serve; ele só fica guardado junto com os seus saves.

Os projetos que você abriu voltam sozinhos, em abas, na próxima vez que abrir o programa.

## O que dá para fazer

- **Salvamento rápido** guarda tudo o que mudou agora, com o nome que você escolher. Se a IA estiver ligada, ela sugere um nome, e você pode editar antes de salvar.
- **Linha do tempo**: cada bolinha é um save. Clique numa delas para **espiar** como o projeto estava naquele momento, arquivo por arquivo, sem mudar nada na sua pasta. **Voltar para Agora** sai do modo de espiar.
- **Trazer de volta**: ao espiar um save, você pode trazer de volta um arquivo ou o projeto inteiro. Isso aparece em **Agora** como alteração não salva, e você decide se faz um novo save.
- **Descartar alterações** joga fora o que você mudou desde o último save. Nada se perde de verdade: tudo vai para a **Recuperação**, e você pode **Desfazer** na hora ou restaurar depois, pela **Recuperação…** (menu do botão direito na aba do projeto), por até 30 dias.
- **Configurações** (a engrenagem no canto): idioma (português ou inglês) e se os nomes dos saves vêm da IA ou de você.

## Linhas do tempo

- **Carregar este save**: ao espiar um save, este botão leva a sua pasta para aquele momento. Se você tiver alterações não salvas, o SaveStates pede para salvar antes. Logo depois aparece **Desfazer**, para voltar para onde você estava.
- Se você carregar um save do meio de uma linha e salvar, esse save começa uma **linha do tempo nova**, numa faixa de outra cor, com o nome do primeiro save dela. A linha antiga continua lá, do jeito que estava.
- Para voltar para uma linha, carregue o último save dela: os próximos saves continuam nela. Se você carregou um save e não salvou nada, a linha nova que ia começar ali some sozinha.
- Para dar outro nome a uma linha, clique duas vezes no nome dela (ou clique com o botão direito e escolha **Renomear**).
- Para tirar da tela uma linha que você não usa mais, clique com o botão direito no nome dela e escolha **Arquivar linha do tempo**. Nada é apagado: ela fica em **Linhas do tempo arquivadas…**, no menu do botão direito na aba do projeto.
- Para apagar de vez uma linha arquivada, abra **Linhas do tempo arquivadas…** e clique em **Apagar…** ao lado dela. Mudou de ideia? Por 30 dias ela pode voltar pela **Recuperação…** (menu do botão direito na aba do projeto), do jeito que estava.
- Arquivos ignorados (como o `.env`) que estejam no caminho de um save que você carrega não se perdem: o SaveStates guarda esses arquivos e eles voltam sozinhos quando você sai daquele save.

Os saves ficam dentro da própria pasta do projeto, numa pasta escondida chamada `.git`. Não apague essa pasta: sem ela, o projeto perde os saves.

## Se aparecer um erro de gráficos

Se o SaveStates não abrir e mostrar a mensagem **"Não foi possível iniciar os gráficos (Vulkan). Atualize o driver da sua placa de vídeo e tente de novo."**, o driver da sua placa de vídeo está velho. Para atualizar:

1. Abra o **Windows Update** (Configurações, Windows Update, Opções avançadas, Atualizações opcionais) e instale as atualizações de driver que aparecerem, ou
2. baixe o driver mais novo no site do fabricante da sua placa: NVIDIA, AMD ou Intel.
3. Reinicie o computador e abra o SaveStates de novo.

## Atualizando para uma versão nova

Rode o instalador novo. Ele troca o programa por cima do antigo, e os seus projetos, saves e configurações continuam como estavam.

## Desinstalando

Abra **Configurações do Windows**, **Aplicativos**, **Aplicativos instalados**, encontre **SaveStates** e clique em **Desinstalar**. Os seus projetos e saves não são apagados. As configurações do programa ficam em `%APPDATA%\SaveStates`, e você pode apagar essa pasta se quiser.

## Se algo der errado

Fale com quem te passou o instalador, e conte o que você estava fazendo e o que apareceu na tela.

O SaveStates anota os problemas que encontra num arquivo chamado `log.txt`, que ajuda quem for resolver. Para achá-lo, abra o Explorador de Arquivos, cole `%APPDATA%\SaveStates` na barra de endereço e aperte Enter. Mande esse arquivo junto quando pedir ajuda.
