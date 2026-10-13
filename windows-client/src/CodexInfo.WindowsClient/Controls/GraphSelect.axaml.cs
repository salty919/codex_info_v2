// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.ComponentModel;
using System.Globalization;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Controls.Templates;
using Avalonia.Data;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.VisualTree;
using CodexInfo.WindowsClient.Core;
using CodexInfo.WindowsClient.Localization;

namespace CodexInfo.WindowsClient.Controls;

public partial class GraphSelect : ToggleButton
{
    private string? openedPeriodId;
    private INotifyCollectionChanged? observedItems;
    private readonly ObservableCollection<PeriodDateGroup> dateGroups = new();
    private readonly Dictionary<string, PeriodDateGroup> groupCache = new(StringComparer.Ordinal);
    private bool updatingPeriods;

    public static readonly StyledProperty<bool> GroupPeriodsByDateProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(GroupPeriodsByDate));
    public bool GroupPeriodsByDate { get => GetValue(GroupPeriodsByDateProperty); set => SetValue(GroupPeriodsByDateProperty, value); }

    public static readonly StyledProperty<string> FieldLabelProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(FieldLabel), "");
    public static readonly StyledProperty<bool> HasFieldLabelProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(HasFieldLabel));
    public static readonly StyledProperty<Thickness> ValueMarginProperty =
        AvaloniaProperty.Register<GraphSelect, Thickness>(nameof(ValueMargin));
    public static readonly StyledProperty<string> ValueTextProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(ValueText), "");
    public static readonly StyledProperty<IEnumerable?> ItemsSourceProperty =
        AvaloniaProperty.Register<GraphSelect, IEnumerable?>(nameof(ItemsSource));
    public static readonly StyledProperty<object?> SelectedItemProperty =
        AvaloniaProperty.Register<GraphSelect, object?>(nameof(SelectedItem), defaultBindingMode: BindingMode.TwoWay);
    public static readonly StyledProperty<IDataTemplate?> ItemTemplateProperty =
        AvaloniaProperty.Register<GraphSelect, IDataTemplate?>(nameof(ItemTemplate));
    public static readonly StyledProperty<string> MenuAutomationIdProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(MenuAutomationId), "");
    public static readonly StyledProperty<double> PopupWidthProperty =
        AvaloniaProperty.Register<GraphSelect, double>(nameof(PopupWidth), 560);
    public static readonly StyledProperty<int> MaxVisibleItemsProperty =
        AvaloniaProperty.Register<GraphSelect, int>(nameof(MaxVisibleItems), 4);
    public static readonly StyledProperty<bool> LimitPopupToFieldProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(LimitPopupToField));
    public static readonly StyledProperty<bool> PopupOnLeftProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(PopupOnLeft));

    public string FieldLabel { get => GetValue(FieldLabelProperty); set => SetValue(FieldLabelProperty, value); }
    public bool HasFieldLabel { get => GetValue(HasFieldLabelProperty); set => SetValue(HasFieldLabelProperty, value); }
    public Thickness ValueMargin { get => GetValue(ValueMarginProperty); set => SetValue(ValueMarginProperty, value); }
    public string ValueText { get => GetValue(ValueTextProperty); set => SetValue(ValueTextProperty, value); }
    public IEnumerable? ItemsSource { get => GetValue(ItemsSourceProperty); set => SetValue(ItemsSourceProperty, value); }
    public object? SelectedItem { get => GetValue(SelectedItemProperty); set => SetValue(SelectedItemProperty, value); }
    public IDataTemplate? ItemTemplate { get => GetValue(ItemTemplateProperty); set => SetValue(ItemTemplateProperty, value); }
    public string MenuAutomationId { get => GetValue(MenuAutomationIdProperty); set => SetValue(MenuAutomationIdProperty, value); }
    public double PopupWidth { get => GetValue(PopupWidthProperty); set => SetValue(PopupWidthProperty, value); }
    public int MaxVisibleItems { get => GetValue(MaxVisibleItemsProperty); set => SetValue(MaxVisibleItemsProperty, value); }
    public bool LimitPopupToField { get => GetValue(LimitPopupToFieldProperty); set => SetValue(LimitPopupToFieldProperty, value); }
    public bool PopupOnLeft { get => GetValue(PopupOnLeftProperty); set => SetValue(PopupOnLeftProperty, value); }

    public bool IsOpen => MenuPopup.IsOpen;

    public GraphSelect()
    {
        InitializeComponent();
        PeriodTree.ItemsSource = dateGroups;
        PeriodTree.DataTemplates.Add(new FuncTreeDataTemplate<PeriodDateGroup>(
            (group, _) => new TextBlock { Text = group!.Label, VerticalAlignment = Avalonia.Layout.VerticalAlignment.Center, MinHeight = 32 },
            group => group!.Periods));
        PeriodTree.DataTemplates.Add(new FuncDataTemplate<ApiHistoryPeriod>(
            (period, _) => ItemTemplate?.Build(period) ?? new TextBlock { Text = period!.Label, MinHeight = 32, VerticalAlignment = Avalonia.Layout.VerticalAlignment.Center }));
        PeriodTree.ContainerPrepared += (_, e) =>
        {
            if (e.Container is TreeViewItem item && item.DataContext is PeriodDateGroup group)
                item.Bind(TreeViewItem.IsExpandedProperty, new Binding(nameof(PeriodDateGroup.IsExpanded)) { Source = group, Mode = BindingMode.TwoWay });
        };
        MenuPopup.PlacementTarget = this;
        MenuPopup.Closed += (_, _) =>
        {
            Chevron.Text = "⌄";
            if (IsChecked == true)
            {
                IsChecked = false;
            }
        };
    }

    protected override void OnPropertyChanged(AvaloniaPropertyChangedEventArgs change)
    {
        base.OnPropertyChanged(change);
        if (PeriodTree is null) return;
        if (change.Property == ItemsSourceProperty)
        {
            if (observedItems is not null) observedItems.CollectionChanged -= OnItemsChanged;
            observedItems = ItemsSource as INotifyCollectionChanged;
            if (observedItems is not null) observedItems.CollectionChanged += OnItemsChanged;
        }
        if (change.Property == ItemsSourceProperty || change.Property == GroupPeriodsByDateProperty)
            RebuildDateGroups();
        if (change.Property == SelectedItemProperty && GroupPeriodsByDate)
        {
            updatingPeriods = true;
            PeriodTree.SelectedItem = SelectedItem;
            updatingPeriods = false;
        }
    }

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        LocalizationService.LanguageChanged += OnPresentationChanged;
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        LocalizationService.LanguageChanged -= OnPresentationChanged;
        base.OnDetachedFromVisualTree(e);
    }

    private void OnPresentationChanged(object? sender, EventArgs e) => RebuildDateGroups();
    private void OnItemsChanged(object? sender, NotifyCollectionChangedEventArgs e) => RebuildDateGroups();

    private void RebuildDateGroups()
    {
        MenuList.IsVisible = !GroupPeriodsByDate;
        PeriodTree.IsVisible = GroupPeriodsByDate;
        if (!GroupPeriodsByDate) return;
        updatingPeriods = true;
        var groups = (ItemsSource?.OfType<ApiHistoryPeriod>() ?? [])
            .GroupBy(period => TimeZoneInfo.ConvertTime(DateTimeOffset.FromUnixTimeSeconds(period.StartAt), LocalizationService.DisplayTimeZone)
                .ToString("yyyy/MM/dd", CultureInfo.InvariantCulture))
            .OrderByDescending(group => group.Key, StringComparer.Ordinal).ToArray();
        dateGroups.Clear();
        foreach (var group in groups)
        {
            if (!groupCache.TryGetValue(group.Key, out var node))
                groupCache[group.Key] = node = new PeriodDateGroup(group.Key);
            node.Periods.Clear();
            foreach (var period in group) node.Periods.Add(period);
            dateGroups.Add(node);
        }
        PeriodTree.SelectedItem = SelectedItem;
        updatingPeriods = false;
    }

    private void OnPeriodSelectionChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (updatingPeriods || e.AddedItems.Count == 0) return;
        if (e.AddedItems[0] is PeriodDateGroup)
        {
            updatingPeriods = true;
            PeriodTree.SelectedItem = SelectedItem;
            updatingPeriods = false;
            return;
        }
        if (e.AddedItems[0] is not ApiHistoryPeriod period) return;
        SelectedItem = period;
        if (MenuPopup.IsOpen && period.Id != openedPeriodId) Close();
    }

    private void OnPeriodPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        var expander = false;
        for (var current = e.Source as Visual; current is not null; current = current.GetVisualParent())
        {
            if (current is ToggleButton) expander = true;
            if (current is TreeViewItem item)
            {
                if (item.DataContext is ApiHistoryPeriod period) { SelectedItem = period; Close(); }
                else if (!expander && item.DataContext is PeriodDateGroup group) group.IsExpanded = !group.IsExpanded;
                break;
            }
            if (ReferenceEquals(current, PeriodTree)) break;
        }
    }

    private void OnFieldClick(object? sender, RoutedEventArgs e)
    {
        if (IsChecked != true || !IsEnabled || ItemsSource is null)
        {
            IsChecked = false;
            ClosePopup();
            return;
        }

        var count = ItemsSource.Cast<object>().Count();
        if (count == 0)
        {
            IsChecked = false;
            ClosePopup();
            return;
        }

        PopupSurface.Width = LimitPopupToField ? Math.Min(PopupWidth, Bounds.Width) : PopupWidth;
        MenuList.Height = Math.Min(count, MaxVisibleItems) * 32;
        PeriodTree.Height = MaxVisibleItems * 32;
        MenuPopup.Placement = PopupOnLeft
            ? PlacementMode.LeftEdgeAlignedTop
            : PlacementMode.BottomEdgeAlignedLeft;
        openedPeriodId = (SelectedItem as ApiHistoryPeriod)?.Id;
        MenuPopup.IsOpen = true;
        Chevron.Text = "⌃";
    }

    private void OnSelectionChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (MenuPopup.IsOpen && e.AddedItems.Count > 0)
        {
            if (e.AddedItems[0] is ApiHistoryPeriod period && period.Id == openedPeriodId)
            {
                return;
            }
            Close();
        }
    }

    private void OnMenuPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        for (var current = e.Source as Visual; current is not null; current = current.GetVisualParent())
        {
            if (current is ListBoxItem item)
            {
                SelectedItem = item.DataContext;
                Close();
                break;
            }
            if (ReferenceEquals(current, MenuList))
            {
                break;
            }
        }
    }

    public void Close()
    {
        if (IsChecked == true)
        {
            IsChecked = false;
        }
        ClosePopup();
    }

    private void ClosePopup()
    {
        MenuPopup.IsOpen = false;
        Chevron.Text = "⌄";
    }
}

public sealed class PeriodDateGroup(string label) : INotifyPropertyChanged
{
    public string Label { get; } = label;
    public ObservableCollection<ApiHistoryPeriod> Periods { get; } = new();
    private bool isExpanded;
    public bool IsExpanded
    {
        get => isExpanded;
        set
        {
            if (isExpanded == value)
            {
                return;
            }

            isExpanded = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsExpanded)));
        }
    }
    public event PropertyChangedEventHandler? PropertyChanged;
}
