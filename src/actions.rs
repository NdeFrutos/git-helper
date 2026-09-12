use gpui::actions;

actions!(
    git_helper,
    [
        OpenRepository,
        CloneRepository,
        CloseActiveRepository,
        NextRepository,
        PreviousRepository,
        RefreshRepository,
        ShowHistory,
        ShowChanges,
        CreateCommit,
        GenerateCommitMessage,
        OpenInEditor,
        OpenTerminalHere,
        RevealInFileManager,
    ]
);
