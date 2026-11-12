# Motioner integrado

O painel Motioner é parte do aplicativo. Ele abre ao iniciar esta edição e também está no menu
**Window / Janela → Motioner**. Pode ser acoplado, movido ou fechado como os outros painéis.
A interface é nativa em Rust/egui: não precisa de extensão CEP, navegador, Electron ou servidor manual.

## PSD: preservar a aparência

Reimporte o PSD para obter as camadas com a aparência preservada.

A importação como composição usa os pixels que o Photoshop salvou para cada camada de texto e
objeto inteligente, quando existem. Assim, a imagem dessas camadas não depende da instalação das
fontes, nem da capacidade de abrir novamente um arquivo embutido ou vinculado. A posição vem dos
limites da imagem salva, evitando aplicar duas vezes a transformação do objeto inteligente.
A opção Retain Layer Sizes continua preservando o tamanho individual das camadas. PSDs sem
registros de camadas usam a imagem mesclada do documento, em vez de criar uma composição vazia.

O conteúdo textual original continua no PSD. Para editar as letras, selecione a camada e use
**Layer → Create → Convert to Editable Text**, também disponível na aba Ferramentas do Motioner.
Essa conversão explícita usa as fontes instaladas e pode mudar a aparência se a fonte estiver ausente.
Camadas de texto sem pixels salvos continuam usando o importador de texto editável.

Isso corrige a perda causada pela reconstrução desnecessária de texto e objetos inteligentes;
não torna o compositor idêntico ao Photoshop para todos os estilos, ajustes ou modos de mesclagem.
Um recurso sem pixels salvos e sem fonte/arquivo disponível não pode ser recuperado do nada.
Para um resultado estático fiel, importe o PSD como footage/imagem mesclada. Para animar camadas,
importe como composição. Salve o PSD no Photoshop com a opção de maximizar compatibilidade quando
precisar garantir que a imagem mesclada esteja incluída.

## Animar e ferramentas

Selecione camadas na timeline, escolha um dos 21 movimentos, ajuste duração, intervalo em frames,
intensidade e entrada/saída e aplique. Os presets são implementações originais desta edição, com
keyframes editáveis, em uma transação que pode ser desfeita. A aplicação parte da agulha para a
entrada e do fim da camada para a saída. Intervalo desloca as camadas selecionadas em sequência.
O comando `motioner.animate` também está disponível para CLI, automação e MCP;
`motioner.presets` lista os identificadores e nomes.

Ferramentas oferece conversão de texto PSD, 3D, nova câmera, nulo controlador e efeitos nativos.
O painel adapta o fluxo do Arcano/Motioner ao editor; não executa o pacote CEP de After Effects.

## Chat local com Claude Code ou Codex

Instale e autentique um dos CLIs oficiais no terminal. Em Ajustes, escolha o assistente, informe
um executável personalizado se a descoberta automática não encontrar a instalação e, opcionalmente,
o modelo. Sem conta própria ou autenticação do Motioner. Os pedidos usam a conta, conexão e limites
do provedor escolhido. Texto do pedido e dados consultados no projeto são enviados a esse provedor.

O `effectcraft-cli` deve permanecer ao lado do aplicativo no pacote portátil. O painel o configura
como servidor MCP do assistente e liga o canal local do projeto aberto apenas no primeiro envio.
A porta é escolhida pelo sistema em 127.0.0.1; não é publicada na rede. O canal usa o protocolo
JSON-lines existente do EffectCraft e não oferece autenticação entre processos do mesmo usuário.
O servidor permanece ativo até fechar o aplicativo. Use **Interromper** para encerrar o pedido atual.
Revise as alterações no editor; elas usam os comandos e o histórico de desfazer do projeto.

As instalações npm exigem Node.js no PATH. A descoberta reconhece wrappers npm dos dois provedores
sem invocar um shell para interpretar o caminho. Instalações recentes podem exigir reiniciar o editor
para que o PATH seja atualizado. O chat local está disponível somente no desktop; a versão web mantém
os presets e informa essa limitação na aba Chat.

## Build

Use Rust 1.95+ e os requisitos descritos em CONTRIBUTING.md:

```sh
cargo build --release -p effectcraft -p effectcraft-cli
```

No Windows, use o toolchain MSVC com as ferramentas C++ do Visual Studio, ou um toolchain GNU
com MinGW recente. Copie os dois executáveis e as licenças para a mesma pasta. Este fork não usa
os certificados de assinatura digital do projeto original. A checagem de atualizações é manual e usa `ferronatin/motioner-app`, evitando que uma atualização
do projeto original substitua o painel integrado. O pacote portátil é uma compilação local,
não um instalador assinado pelo ArtCraft.
